// @okf-doc: /decisions/0012-workspace-mode.md
//! The workspace: its root, root-relative paths, and git ignore rules.
//!
//! A [`Workspace`] is rooted at the enclosing git work tree of the path it
//! was opened on, or at that directory itself when there is no repository
//! (ADR 0009). Directory listings hide the `.git` directory and anything git
//! ignores, using the repository's own exclude stack, so the tree pane and the
//! picker agree with `git status` on what exists.
//!
//! [`ComparisonEndpoint`] and [`Workspace::compare`] provide one direct,
//! repository-wide pair delta. Commit endpoints are resolved and pinned to
//! object IDs; [`ComparisonEndpoint::WorkingTree`] is the final on-disk
//! state, and [`ComparisonEndpoint::Index`] remains an explicit mutable
//! layer.
//!
//! # Examples
//!
//! ```no_run
//! use fathomable_core::workspace::Workspace;
//!
//! let mut workspace = Workspace::discover(".")?;
//! for entry in workspace.list_dir("")? {
//!     println!("{}{}", entry.name(), if entry.is_dir() { "/" } else { "" });
//! }
//! # Ok::<(), fathomable_core::workspace::WorkspaceError>(())
//! ```

use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use gix::ObjectId;
use gix::bstr::{BString, ByteSlice};
use gix::revision::walk::Sorting;
use gix::traverse::commit::simple::CommitTimeOrder;
use gix::worktree::stack::state::attributes::Source as AttrSource;
use gix::worktree::stack::state::ignore::Source;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::annotations::FullFileDigest;
use crate::content::{self, Attr};
use crate::diff::{Comparison, Diff, FileMode, PathChange, PathInfo, PathState};
use crate::status::{self, Changes, State, Status};

/// Cooperative cancellation of a workspace scan, without joining its thread.
#[derive(Debug, Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    /// Ask the scan to stop before its next entry or content read.
    pub fn cancel(&self) {
        self.0.store(true, AtomicOrdering::Relaxed);
    }

    /// Whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(AtomicOrdering::Relaxed)
    }
}

/// Bounded discovery results, including why enumeration stopped early.
#[derive(Debug)]
pub struct Discovery {
    paths: Vec<String>,
    incomplete: Option<WorkspaceError>,
}

impl Discovery {
    /// Discovered root-relative file paths.
    #[must_use]
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// The limit, cancellation, or filesystem error preventing complete coverage.
    #[must_use]
    pub fn incomplete(&self) -> Option<&WorkspaceError> {
        self.incomplete.as_ref()
    }
}

/// One directory entry, as the tree pane shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    name: String,
    is_dir: bool,
    is_link: bool,
}

impl Entry {
    /// The file name, without any directory part.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the entry is browsable as a directory; a symlink to one
    /// counts, though git and the dirty set treat any symlink as a file.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    /// Whether the entry is a symlink, wherever it points.
    #[must_use]
    pub fn is_symlink(&self) -> bool {
        self.is_link
    }
}

/// Whether a path is a file or a directory, for ignore evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKind {
    /// A regular file or a symlink, wherever the symlink points.
    File,
    /// A directory.
    Dir,
}

/// Which entries a listing includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Filter {
    /// Hide `.git` and everything git ignores.
    #[default]
    Visible,
    /// Hide only `.git`.
    All,
}

/// One repository commit discovered or resolved by the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    hex: String,
    time: u64,
    subject: String,
    parents: Vec<String>,
}

impl Commit {
    /// The full commit id as hex.
    #[must_use]
    pub fn hex(&self) -> &str {
        &self.hex
    }

    /// The immutable object identity of this commit.
    #[must_use]
    pub fn id(&self) -> CommitId {
        CommitId(self.hex.clone())
    }

    /// The first seven characters of the id.
    #[must_use]
    pub fn short(&self) -> &str {
        &self.hex[..self.hex.len().min(7)]
    }

    /// The committer time in seconds since the Unix epoch.
    #[must_use]
    pub fn time(&self) -> u64 {
        self.time
    }

    /// The first line of the message.
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The commit's direct parents, in Git order.
    #[must_use]
    pub fn parents(&self) -> impl ExactSizeIterator<Item = CommitId> + '_ {
        self.parents.iter().cloned().map(CommitId)
    }
}

/// One bounded regular blob loaded from an immutable commit tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitBlob {
    mode: FileMode,
    size: u64,
    object: String,
    bytes: Vec<u8>,
}

struct CommitBlobSource {
    repo: gix::Repository,
    mode: FileMode,
    size: u64,
    object: ObjectId,
}

impl CommitBlob {
    /// The regular or executable Git file mode.
    #[must_use]
    pub const fn mode(&self) -> FileMode {
        self.mode
    }

    /// The uncompressed byte size verified from the object header.
    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// The canonical Git blob object ID.
    #[must_use]
    pub fn object(&self) -> &str {
        &self.object
    }

    /// The raw Git blob bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Consume the blob and return its raw bytes.
    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// A validated immutable Git commit object ID.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommitId(String);

impl CommitId {
    /// Parse a full hexadecimal Git object ID.
    ///
    /// # Errors
    ///
    /// Returns [`CommitIdError`] when `value` is not a full SHA-1 object ID.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, CommitIdError> {
        let value = value.as_ref();
        if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CommitIdError {
                value: value.to_owned(),
            });
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    /// The full hexadecimal object ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first seven hexadecimal characters.
    #[must_use]
    pub fn short(&self) -> &str {
        &self.0[..7]
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A commit ID that could not be parsed as a full object ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitIdError {
    value: String,
}

impl fmt::Display for CommitIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid commit object ID `{}`", self.value)
    }
}

impl std::error::Error for CommitIdError {}

/// Immutable identity of one checkout and its shared repository.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckoutIdentity {
    checkout: PathBuf,
    repository: PathBuf,
}

impl CheckoutIdentity {
    pub(crate) fn from_canonical_paths(checkout: PathBuf, repository: PathBuf) -> Self {
        Self {
            checkout,
            repository,
        }
    }

    /// The canonical checkout root.
    #[must_use]
    pub fn checkout(&self) -> &Path {
        &self.checkout
    }

    /// The canonical repository key shared by linked worktrees.
    #[must_use]
    pub fn repository(&self) -> &Path {
        &self.repository
    }

    pub(crate) fn is_valid(&self) -> bool {
        self.checkout.is_absolute() && self.repository.is_absolute()
    }
}

/// A typed observation of the active checkout's `HEAD`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadState {
    /// A symbolic reference resolved to one full commit ID.
    Symbolic {
        /// Full Git reference name.
        reference: String,
        /// Commit currently named by the reference.
        commit: CommitId,
    },
    /// A directly checked-out commit.
    Detached {
        /// The checked-out commit.
        commit: CommitId,
    },
    /// A symbolic `HEAD` whose reference has no commit yet.
    Unborn,
    /// `HEAD` could not be observed reliably.
    Unavailable {
        /// Stable diagnostic suitable for logs and UI status.
        error: String,
    },
}

impl HeadState {
    /// The observed commit, when `HEAD` names one.
    #[must_use]
    pub const fn commit(&self) -> Option<&CommitId> {
        match self {
            Self::Symbolic { commit, .. } | Self::Detached { commit } => Some(commit),
            Self::Unborn | Self::Unavailable { .. } => None,
        }
    }
}

/// One generation-qualified `HEAD` observation for a checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadObservation {
    generation: u64,
    checkout: CheckoutIdentity,
    state: HeadState,
}

impl HeadObservation {
    /// Comparison generation associated with this observation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Checkout and repository observed.
    #[must_use]
    pub const fn checkout(&self) -> &CheckoutIdentity {
        &self.checkout
    }

    /// Typed `HEAD` state.
    #[must_use]
    pub const fn state(&self) -> &HeadState {
        &self.state
    }
}

/// A generation-qualified change between two active-checkout observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadTransition {
    generation: u64,
    checkout: CheckoutIdentity,
    previous: HeadState,
    current: HeadState,
}

impl HeadTransition {
    /// Construct a transition when both observations belong to one checkout.
    #[must_use]
    pub fn between(previous: &HeadObservation, current: &HeadObservation) -> Option<Self> {
        (previous.checkout == current.checkout && previous.state != current.state).then(|| Self {
            generation: current.generation,
            checkout: current.checkout.clone(),
            previous: previous.state.clone(),
            current: current.state.clone(),
        })
    }

    /// Generation in which the transition was observed.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Checkout whose `HEAD` moved.
    #[must_use]
    pub const fn checkout(&self) -> &CheckoutIdentity {
        &self.checkout
    }

    /// Previous typed `HEAD` state.
    #[must_use]
    pub const fn previous(&self) -> &HeadState {
        &self.previous
    }

    /// Current typed `HEAD` state.
    #[must_use]
    pub const fn current(&self) -> &HeadState {
        &self.current
    }
}

/// One immutable path expectation for exact commit reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactFile {
    checkout: CheckoutIdentity,
    path: PathBuf,
    digest: FullFileDigest,
}

impl ExactFile {
    /// Construct a named exact-file expectation.
    #[must_use]
    pub fn new(
        checkout: CheckoutIdentity,
        path: impl Into<PathBuf>,
        digest: FullFileDigest,
    ) -> Self {
        Self {
            checkout,
            path: path.into(),
            digest,
        }
    }

    /// Checkout and repository allowed to verify this expectation.
    #[must_use]
    pub const fn checkout(&self) -> &CheckoutIdentity {
        &self.checkout
    }

    /// Repository-relative path to verify.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Expected full-file identity.
    #[must_use]
    pub const fn digest(&self) -> &FullFileDigest {
        &self.digest
    }
}

/// Exact result for one commit reconciliation candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExactFileMatch {
    /// The regular commit blob has exactly the expected digest and length.
    Match,
    /// The path is absent from the commit.
    Missing,
    /// The regular commit blob has different bytes.
    Mismatch,
    /// The path or object could not be verified.
    Unavailable(String),
}

/// One result from a bounded exact commit reconciliation job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactFileResult {
    path: PathBuf,
    result: ExactFileMatch,
}

impl ExactFileResult {
    /// Candidate path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Exact verification result.
    #[must_use]
    pub const fn result(&self) -> &ExactFileMatch {
        &self.result
    }
}

/// Progress made by one bounded exact-file verification batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactFileProgress {
    /// Every candidate was examined.
    Complete,
    /// Aggregate bytes were exhausted before this candidate.
    ContinueAt {
        /// Candidate index at which a fresh aggregate budget must resume.
        next: usize,
    },
}

/// Results and typed continuation for exact commit-file verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactFileBatch {
    results: Vec<ExactFileResult>,
    progress: ExactFileProgress,
}

impl ExactFileBatch {
    /// Examined candidate results.
    #[must_use]
    pub fn results(&self) -> &[ExactFileResult] {
        &self.results
    }

    /// Whether and where verification must continue with a fresh budget.
    #[must_use]
    pub const fn progress(&self) -> ExactFileProgress {
        self.progress
    }

    /// Consume the batch into its results and continuation.
    #[must_use]
    pub fn into_parts(self) -> (Vec<ExactFileResult>, ExactFileProgress) {
        (self.results, self.progress)
    }
}

fn exact_file_result(
    candidate: &ExactFile,
    loaded: &Result<Option<FullFileDigest>, String>,
) -> ExactFileResult {
    ExactFileResult {
        path: candidate.path.clone(),
        result: match loaded {
            Ok(Some(actual)) if actual == candidate.digest() => ExactFileMatch::Match,
            Ok(Some(_)) => ExactFileMatch::Mismatch,
            Ok(None) => ExactFileMatch::Missing,
            Err(error) => ExactFileMatch::Unavailable(error.clone()),
        },
    }
}

/// Why a coherent Git index manifest is unavailable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexManifestUnavailable {
    /// At least one path has unresolved conflict stages.
    Conflict {
        /// Raw path carrying conflict stages.
        path: Box<[u8]>,
    },
    /// At least one path is only an intent-to-add placeholder.
    IntentToAdd {
        /// Raw path carrying the placeholder.
        path: Box<[u8]>,
    },
    /// Sparse-directory entries cannot be used as a complete tree.
    Sparse,
    /// An entry uses a mode or path shape this build cannot interpret.
    Unsupported {
        /// Raw path, or empty when the condition is index-wide.
        path: Box<[u8]>,
        /// Reason the entry cannot form an exact tree.
        detail: String,
    },
}

/// One raw tree-relevant entry from a captured Git index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexManifestEntry {
    path: Box<[u8]>,
    mode: FileMode,
    object: String,
    assume_unchanged: bool,
    skip_worktree: bool,
}

impl IndexManifestEntry {
    /// Canonical slash-separated raw Git path bytes.
    #[must_use]
    pub fn path_bytes(&self) -> &[u8] {
        &self.path
    }

    /// Git tree mode represented by this entry.
    #[must_use]
    pub const fn mode(&self) -> FileMode {
        self.mode
    }

    /// Exact object ID represented by this entry.
    #[must_use]
    pub fn object(&self) -> &str {
        &self.object
    }

    /// Whether Git may assume the worktree copy is unchanged.
    #[must_use]
    pub const fn assume_unchanged(&self) -> bool {
        self.assume_unchanged
    }

    /// Whether sparse checkout normally skips the worktree copy.
    #[must_use]
    pub const fn skip_worktree(&self) -> bool {
        self.skip_worktree
    }
}

/// One bounded, immutable snapshot of the complete Git index tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexManifest {
    checkout: CheckoutIdentity,
    identity: String,
    entries: Box<[IndexManifestEntry]>,
}

impl IndexManifest {
    /// Checkout and repository from which the index was captured.
    #[must_use]
    pub const fn checkout(&self) -> &CheckoutIdentity {
        &self.checkout
    }

    /// Full SHA-256 over canonical length-delimited entries.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Every tree-relevant index entry in raw-path order.
    #[must_use]
    pub fn entries(&self) -> &[IndexManifestEntry] {
        &self.entries
    }

    /// Find one captured entry by canonical raw path bytes.
    #[must_use]
    pub fn entry(&self, path: &[u8]) -> Option<&IndexManifestEntry> {
        self.entries
            .binary_search_by(|entry| entry.path.as_ref().cmp(path))
            .ok()
            .map(|index| &self.entries[index])
    }
}

/// Result of attempting to capture a complete index tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexManifestCapture {
    /// The complete bounded index was captured.
    Available(IndexManifest),
    /// The index cannot safely stand for a complete Git tree.
    Unavailable(IndexManifestUnavailable),
}

/// The kind of named revision offered to a viewer picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RevisionChoiceKind {
    /// A local branch under `refs/heads`.
    Branch,
    /// A local tag under `refs/tags`.
    Tag,
}

impl fmt::Display for RevisionChoiceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Branch => "branch",
            Self::Tag => "tag",
        })
    }
}

/// A named branch or tag resolved to an immutable commit ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RevisionChoice {
    kind: RevisionChoiceKind,
    name: String,
    commit: CommitId,
    remote: bool,
}

impl RevisionChoice {
    /// Whether this choice is a branch or tag.
    #[must_use]
    pub const fn kind(&self) -> RevisionChoiceKind {
        self.kind
    }

    /// The branch or tag name without its `refs/*` prefix.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The pinned commit resolved from this name.
    #[must_use]
    pub const fn commit(&self) -> &CommitId {
        &self.commit
    }

    /// Whether this branch is a remote-tracking branch.
    #[must_use]
    pub const fn is_remote_branch(&self) -> bool {
        matches!(self.kind, RevisionChoiceKind::Branch) && self.remote
    }
}

impl fmt::Display for RevisionChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_remote_branch() {
            write!(f, "remote branch {} ({})", self.name, self.commit.short())
        } else {
            write!(f, "{} {} ({})", self.kind, self.name, self.commit.short())
        }
    }
}

/// An immutable or mutable side of a repository comparison.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ComparisonEndpoint {
    /// The empty tree, used before a root commit or in an empty repository.
    EmptyTree,
    /// A commit pinned to this resolved object ID.
    Commit(CommitId),
    /// An explicit saved review point, resolved by its owning store.
    ReviewPoint(String),
    /// The current Git index.
    Index,
    /// The final current on-disk state, including eligible additions and deletions.
    WorkingTree,
}

impl fmt::Display for ComparisonEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTree => f.write_str("empty tree"),
            Self::Commit(id) => f.write_str(id.short()),
            Self::ReviewPoint(id) => {
                let preview: String = id.chars().take(12).collect();
                write!(f, "review point {preview}")
            }
            Self::Index => f.write_str("index"),
            Self::WorkingTree => f.write_str("working tree"),
        }
    }
}

/// Metadata for an endpoint path, shared with review-point capture.
#[derive(Debug, Clone)]
pub(crate) struct EndpointFile {
    pub(crate) info: PathInfo,
}

/// How far below the oldest wanted commit's committer time a reach walk
/// still descends, in seconds.
///
/// A commit's ancestors are normally older than it, so the walk could
/// stop at that time exactly; the slack absorbs the clock skew between
/// the machines that made the commits. Setting it too low hides a thread
/// whose commit sits under a mis-dated ancestor; setting it high only
/// walks more of the history a rewrite orphaned.
const REACH_SLACK: gix::date::SecondsSinceUnixEpoch = 7 * 24 * 60 * 60;

/// Largest commit or tree metadata object accepted by exact source reads.
///
/// This matches the MCP selected-source body budget. Larger Git metadata is
/// not needed for review and must be rejected before `gix` allocates it.
const MAX_EXACT_METADATA_BYTES: u64 = 64 * 1_024 * 1_024;

/// The committer time of the commit `hex` names, `None` when the object
/// store does not hold such a commit.
fn commit_time(repo: &gix::Repository, hex: &str) -> Option<gix::date::SecondsSinceUnixEpoch> {
    let id = ObjectId::from_hex(hex.as_bytes()).ok()?;
    let commit = repo.find_object(id).ok()?.try_into_commit().ok()?;
    commit.time().ok().map(|time| time.seconds)
}

/// A workspace root with git ignore evaluation.
pub struct Workspace {
    root: PathBuf,
    name: String,
    /// What the state directory is keyed by (ADR 0070): the canonical
    /// git common dir, or the root outside git.
    key: PathBuf,
    ignore: Option<Ignore>,
    limits: crate::config::LimitsConfig,
    cancellation: Cancellation,
    content_remaining: Cell<Option<u64>>,
    listing_incomplete: Option<WorkspaceError>,
    listing_examined: Option<usize>,
}

struct Ignore {
    repo: gix::Repository,
    /// Ignore rules and attributes together, one stack for both queries.
    stack: gix::worktree::Stack,
    /// Reused match scratch for the `diff` attribute (ADR 0026).
    diff_attr: gix::attrs::search::Outcome,
}

fn workspace_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl fmt::Debug for Workspace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Workspace")
            .field("root", &self.root)
            .field("name", &self.name)
            .field("limits", &self.limits)
            .field("key", &self.key)
            .field("git", &self.ignore.is_some())
            .finish_non_exhaustive()
    }
}

impl Workspace {
    /// Open the workspace around `path` (a file or directory).
    ///
    /// The root is the enclosing git work tree when one exists, otherwise
    /// `path` itself (or its parent when `path` is a file).
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when `path` does not exist or the
    /// repository's ignore configuration cannot be read.
    pub fn discover(path: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let path = path.as_ref();
        let canonical = path.canonicalize().map_err(|source| WorkspaceError {
            path: path.to_path_buf(),
            message: format!("cannot resolve path: {source}"),
        })?;
        let dir = if canonical.is_dir() {
            canonical.clone()
        } else {
            canonical
                .parent()
                .map_or_else(|| PathBuf::from("/"), Path::to_path_buf)
        };
        match gix::discover_opts(
            &dir,
            gix::discover::upwards::Options::default(),
            open_options(),
        ) {
            Ok(repo) => match repo.workdir() {
                Some(root) => {
                    let root = root.to_path_buf();
                    let ignore = Ignore::new(&repo).map_err(|message| WorkspaceError {
                        path: root.clone(),
                        message,
                    })?;
                    let key = crate::worktrees::canonical(repo.common_dir());
                    let main = repo.main_repo().map_err(|error| WorkspaceError {
                        path: key.clone(),
                        message: format!("cannot open shared repository: {error}"),
                    })?;
                    let name = workspace_name(main.workdir().unwrap_or(main.common_dir()));
                    tracing::info!(root = %root.display(), key = %key.display(), "workspace is a git work tree");
                    Ok(Self {
                        root,
                        name,
                        key,
                        ignore: Some(ignore),
                        limits: crate::config::LimitsConfig::default(),
                        cancellation: Cancellation::default(),
                        content_remaining: Cell::new(None),
                        listing_incomplete: None,
                        listing_examined: None,
                    })
                }
                None => Ok(Self::plain(dir)),
            },
            Err(error) => {
                tracing::debug!(%error, dir = %dir.display(), "no git repository; plain workspace");
                Ok(Self::plain(dir))
            }
        }
    }

    fn plain(root: PathBuf) -> Self {
        tracing::info!(root = %root.display(), "workspace is a plain directory");
        Self {
            name: workspace_name(&root),
            key: root.clone(),
            root,
            ignore: None,
            limits: crate::config::LimitsConfig::default(),
            cancellation: Cancellation::default(),
            content_remaining: Cell::new(None),
            listing_incomplete: None,
            listing_examined: None,
        }
    }

    /// Configure finite discovery and comparison budgets for this workspace.
    pub fn set_limits(&mut self, limits: crate::config::LimitsConfig) {
        self.limits = limits;
    }

    /// The finite budgets used by this workspace.
    #[must_use]
    pub fn limits(&self) -> &crate::config::LimitsConfig {
        &self.limits
    }

    /// Why a materialized directory listing is incomplete, if any.
    #[must_use]
    pub fn listing_incomplete(&self) -> Option<&WorkspaceError> {
        self.listing_incomplete.as_ref()
    }

    pub(crate) fn clear_listing_issue(&mut self) {
        self.listing_incomplete = None;
    }

    pub(crate) fn note_listing_limit(&mut self) {
        self.listing_incomplete = Some(self.scan_error(
            "directory listing limited by retained-path budget; coverage is incomplete",
        ));
    }

    pub(crate) fn begin_listing_scan(&mut self) {
        self.listing_examined = Some(0);
        self.clear_listing_issue();
    }

    pub(crate) fn end_listing_scan(&mut self) {
        self.listing_examined = None;
    }

    /// Attach cooperative cancellation to scans performed by this workspace.
    pub fn set_cancellation(&mut self, cancellation: Cancellation) {
        self.cancellation = cancellation;
    }

    fn scan_error(&self, message: impl Into<String>) -> WorkspaceError {
        WorkspaceError {
            path: self.root.clone(),
            message: message.into(),
        }
    }

    pub(crate) fn check_scan(&self) -> Result<(), WorkspaceError> {
        if self.cancellation.is_cancelled() {
            return Err(self.scan_error("scan cancelled; coverage is incomplete"));
        }
        Ok(())
    }

    pub(crate) fn check_path_count(&self, count: usize) -> Result<(), WorkspaceError> {
        self.check_scan()?;
        if count > self.limits.comparison_path_limit() {
            return Err(
                self.scan_error("comparison limited by path budget; coverage is incomplete")
            );
        }
        Ok(())
    }

    pub(crate) fn begin_comparison(&self) {
        self.content_remaining
            .set(Some(self.limits.comparison_bytes));
    }

    pub(crate) fn content_limit(&self) -> u64 {
        self.content_remaining
            .get()
            .unwrap_or(self.limits.comparison_bytes)
    }

    pub(crate) fn charge_content(&self, bytes: usize) -> Result<(), WorkspaceError> {
        self.check_scan()?;
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        if bytes > self.content_limit() {
            return Err(self
                .scan_error("comparison limited by content byte budget; coverage is incomplete"));
        }
        if let Some(remaining) = self.content_remaining.get() {
            self.content_remaining.set(Some(remaining - bytes));
        }
        Ok(())
    }

    /// The absolute workspace root: the worktree this instance reads.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The shared repository's display name, or the directory name outside Git.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What the workspace's state is keyed by (ADR 0070): the canonical
    /// git common dir, shared by every worktree of the repository, or
    /// the root itself outside git. Pass it where [`XdgDirs`] asks for a
    /// key.
    ///
    /// [`XdgDirs`]: crate::XdgDirs
    #[must_use]
    pub fn key(&self) -> &Path {
        &self.key
    }

    /// Immutable identity of this checkout and its shared repository.
    #[must_use]
    pub fn identity(&self) -> CheckoutIdentity {
        CheckoutIdentity::from_canonical_paths(self.root.clone(), self.key.clone())
    }

    /// Every worktree of the repository (ADR 0070): the main one first,
    /// then the linked ones as git keeps them, or nothing outside git.
    ///
    /// Discovery opens the established common directory independently of this
    /// checkout, so removing the active linked worktree does not lose its peers.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] if the repository, registry, or worktree
    /// metadata cannot be read, including lock reasons larger than 4096 bytes.
    pub fn worktrees(&self) -> Result<Vec<crate::worktrees::Worktree>, WorkspaceError> {
        if !self.is_git() {
            return Ok(Vec::new());
        }
        let repo = gix::open_opts(&self.key, open_options())
            .map_err(|error| self.scan_error(format!("cannot open shared repository: {error}")))?;
        if crate::worktrees::canonical(repo.common_dir()) != self.key {
            return Err(self.scan_error("shared repository identity changed"));
        }
        crate::worktrees::list(&repo)
            .map_err(|error| self.scan_error(format!("cannot list worktrees: {error}")))
    }

    /// The paths a viewer watches for the worktree set and the other
    /// worktrees' `HEAD`s moving (ADR 0070): the active git dir, common
    /// dir, its `refs`, and its `worktrees/` registry. The viewer discovers
    /// registry children with its bounded watcher; none are outside git.
    #[must_use]
    pub fn worktree_watch_paths(&self) -> Vec<PathBuf> {
        let Some(git) = self.ignore.as_ref() else {
            return Vec::new();
        };
        let common = self.key.clone();
        let registry = crate::worktrees::registry(&common);
        let mut out = Vec::with_capacity(4);
        for path in [
            crate::worktrees::canonical(git.repo.git_dir()),
            common.clone(),
            common.join("refs"),
            registry,
        ] {
            if path.is_dir() && !out.contains(&path) {
                out.push(path);
            }
        }
        out
    }

    /// Re-read the ignore rules and attributes.
    ///
    /// The root's `.gitignore`, `.git/info/exclude`, and the
    /// `.gitattributes` beside them are read once when the workspace
    /// opens and kept, so an edit to one of them is not seen until this
    /// runs; call it when a file [`is_rules_file`] names changes. Nothing
    /// happens outside git.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the rules cannot be read; the ones
    /// in use stay as they were.
    pub fn reload_rules(&mut self) -> Result<(), WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Ok(());
        };
        let ignore = Ignore::new(&git.repo).map_err(|message| WorkspaceError {
            path: self.root.clone(),
            message,
        })?;
        self.ignore = Some(ignore);
        tracing::debug!("ignore rules and attributes reloaded");
        Ok(())
    }

    /// Whether the root is inside a git repository.
    #[must_use]
    pub fn is_git(&self) -> bool {
        self.ignore.is_some()
    }

    /// The `HEAD` commit as hex, or `None` outside git or before the first
    /// commit (ADR 0024).
    #[must_use]
    pub fn head_commit(&self) -> Option<String> {
        let git = self.ignore.as_ref()?;
        match git.repo.head_id() {
            Ok(id) => Some(id.to_hex().to_string()),
            Err(error) => {
                tracing::debug!(%error, "no HEAD commit");
                None
            }
        }
    }

    /// Observe `HEAD` without conflating read failures with an unborn branch.
    #[must_use]
    pub fn observe_head(&self, generation: u64) -> HeadObservation {
        let state = self.ignore.as_ref().map_or_else(
            || HeadState::Unavailable {
                error: "HEAD is unavailable outside a Git repository".to_owned(),
            },
            |git| match git.repo.head() {
                Err(error) => HeadState::Unavailable {
                    error: format!("cannot read HEAD: {error}"),
                },
                Ok(head) if head.is_unborn() => HeadState::Unborn,
                Ok(head) => {
                    let detached = head.is_detached();
                    let reference = head.referent_name().map(ToString::to_string);
                    match head.id().map(gix::Id::detach) {
                        None => HeadState::Unavailable {
                            error: "born HEAD has no object ID".to_owned(),
                        },
                        Some(id) => {
                            if let Err(error) = check_commit_header(&git.repo, id) {
                                HeadState::Unavailable {
                                    error: format!("HEAD target is unavailable: {error}"),
                                }
                            } else {
                                let commit = CommitId(id.to_hex().to_string());
                                if detached {
                                    HeadState::Detached { commit }
                                } else if let Some(reference) = reference {
                                    HeadState::Symbolic { reference, commit }
                                } else {
                                    HeadState::Unavailable {
                                        error: "symbolic HEAD has no reference name".to_owned(),
                                    }
                                }
                            }
                        }
                    }
                }
            },
        );
        HeadObservation {
            generation,
            checkout: self.identity(),
            state,
        }
    }

    /// Resolve a local branch, tag, `HEAD`, object ID, or simple parent
    /// expression to one immutable commit.
    ///
    /// Resolution reads local Git metadata only. It never fetches, checks
    /// out, updates refs, or substitutes the current working file.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when this workspace is not Git, the
    /// revision is invalid, the object is unavailable, or it is not a
    /// commit.
    pub fn resolve_revision(&self, revision: impl AsRef<str>) -> Result<Commit, WorkspaceError> {
        let revision = revision.as_ref().trim();
        if revision.is_empty() {
            return Err(self.revision_error(revision, "revision is empty"));
        }
        let Some(git) = self.ignore.as_ref() else {
            return Err(self.revision_error(
                revision,
                "revision resolution is unavailable outside a Git repository",
            ));
        };
        let id = self.resolve_revision_id(&git.repo, revision)?;
        commit_record(&git.repo, id).map_err(|message| self.revision_error(revision, &message))
    }

    /// Load exactly the commit object named by `id`.
    ///
    /// Unlike [`Workspace::resolve_revision`], this does not interpret the ID
    /// as a reference, peel tag objects, or follow Git replacement objects.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when this workspace is not Git, the object
    /// is unavailable, corrupt, or oversized, or the exact object is not a
    /// commit.
    pub fn commit(&self, id: &CommitId) -> Result<Commit, WorkspaceError> {
        let repo = self.exact_repository(id.as_str(), MAX_EXACT_METADATA_BYTES)?;
        let object_id = ObjectId::from_hex(id.as_str().as_bytes()).map_err(|error| {
            self.revision_error(id.as_str(), &format!("invalid commit object ID: {error}"))
        })?;
        check_commit_header(&repo, object_id)
            .and_then(|()| commit_record(&repo, object_id))
            .map_err(|message| self.revision_error(id.as_str(), &message))
    }

    /// Pin `HEAD` to its exact commit without peeling or following replacements.
    ///
    /// This reads `HEAD` once, follows symbolic references, and validates the
    /// original object. Viewer revision resolution remains separate.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] outside Git, for unborn or unreadable `HEAD`,
    /// or when its original target is unavailable, corrupt, oversized, or not
    /// a commit.
    pub fn exact_head_commit(&self) -> Result<Commit, WorkspaceError> {
        let repo = self.exact_repository("HEAD", MAX_EXACT_METADATA_BYTES)?;
        let mut head = repo
            .find_reference("HEAD")
            .map_err(|error| self.revision_error("HEAD", &error.to_string()))?;
        let id = head
            .follow_to_object()
            .map_err(|error| self.revision_error("HEAD", &error.to_string()))?
            .detach();
        check_commit_header(&repo, id)
            .and_then(|()| commit_record(&repo, id))
            .map_err(|message| self.revision_error("HEAD", &message))
    }

    fn exact_repository(
        &self,
        revision: &str,
        max_object_bytes: u64,
    ) -> Result<gix::Repository, WorkspaceError> {
        let git = self.ignore.as_ref().ok_or_else(|| {
            self.revision_error(
                revision,
                "exact commit lookup is unavailable outside a Git repository",
            )
        })?;
        let mut repo = gix::open_opts(
            git.repo.git_dir(),
            exact_open_options(max_object_bytes).open_path_as_is(true),
        )
        .map_err(|error| {
            self.revision_error(
                revision,
                &format!("cannot reopen exact object store: {error}"),
            )
        })?;
        // Keep viewer replacement semantics intact. Configuration must not
        // redirect exact reads.
        repo.objects.ignore_replacements = true;
        Ok(repo)
    }

    /// Enumerate branches and tags resolved to pinned commit IDs.
    ///
    /// Lightweight and annotated tags are both peeled to the commit they
    /// name. Local and remote-tracking branches are included without fetching.
    /// Choices are ordered by kind, then display name, then commit ID.
    /// Typed revisions and recent commits remain available through
    /// [`Workspace::resolve_revision`] and [`Workspace::recent_commits`].
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when reference enumeration fails.
    /// Names that do not resolve to commits are omitted from the picker.
    pub fn revision_choices(&self) -> Result<Vec<RevisionChoice>, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Ok(Vec::new());
        };
        let references = git.repo.references().map_err(|error| WorkspaceError {
            path: self.root.clone(),
            message: format!("cannot enumerate local revisions: {error}"),
        })?;
        let mut choices = Vec::new();
        let branches = references
            .local_branches()
            .map_err(|error| WorkspaceError {
                path: self.root.clone(),
                message: format!("cannot enumerate local branches: {error}"),
            })?;
        for reference in branches {
            let mut reference = reference.map_err(|error| WorkspaceError {
                path: self.root.clone(),
                message: format!("cannot read a local branch reference: {error}"),
            })?;
            if let Some(choice) =
                named_revision_choice(&mut reference, RevisionChoiceKind::Branch, false)
            {
                choices.push(choice);
            }
        }
        let remote_branches = references
            .remote_branches()
            .map_err(|error| WorkspaceError {
                path: self.root.clone(),
                message: format!("cannot enumerate remote branches: {error}"),
            })?;
        for reference in remote_branches {
            let mut reference = reference.map_err(|error| WorkspaceError {
                path: self.root.clone(),
                message: format!("cannot read a remote branch reference: {error}"),
            })?;
            if let Some(choice) =
                named_revision_choice(&mut reference, RevisionChoiceKind::Branch, true)
                && !choice.name.ends_with("/HEAD")
            {
                choices.push(choice);
            }
        }
        let tags = references.tags().map_err(|error| WorkspaceError {
            path: self.root.clone(),
            message: format!("cannot enumerate local tags: {error}"),
        })?;
        for reference in tags {
            let mut reference = reference.map_err(|error| WorkspaceError {
                path: self.root.clone(),
                message: format!("cannot read a local tag reference: {error}"),
            })?;
            if let Some(choice) =
                named_revision_choice(&mut reference, RevisionChoiceKind::Tag, false)
            {
                choices.push(choice);
            }
        }
        choices.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.commit.cmp(&right.commit))
        });
        Ok(choices)
    }

    /// Discover bounded commits reachable from `HEAD`, newest first.
    ///
    /// `offset` and `limit` support paging without restricting later
    /// resolution of an older revision by its branch, tag, or object ID.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the repository history cannot be
    /// read. An unborn repository returns an empty page.
    pub fn recent_commits(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Commit>, WorkspaceError> {
        if self.head_commit().is_none() {
            return Ok(Vec::new());
        }
        self.commits_from("HEAD", offset, limit)
    }

    /// Discover commits reachable from `revision`, newest first.
    ///
    /// The walk is local and bounded by `offset` and `limit`. It never fetches
    /// or mutates references.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when `revision` cannot be resolved or its
    /// history cannot be read.
    pub fn commits_from(
        &self,
        revision: impl AsRef<str>,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Commit>, WorkspaceError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let commit = self.resolve_revision(revision.as_ref())?;
        let tip = ObjectId::from_hex(commit.hex().as_bytes()).map_err(|error| {
            self.revision_error(
                revision.as_ref(),
                &format!("resolved commit ID is invalid: {error}"),
            )
        })?;
        self.walk_commits([tip], offset, Some(limit), None)
    }

    /// Find reachable commits whose object IDs start with `prefix`.
    ///
    /// Every local branch, remote-tracking branch, and tag contributes a walk
    /// tip. Commit objects are decoded only after their IDs match.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when `prefix` is not hexadecimal or local
    /// reference/history enumeration fails.
    pub fn commits_matching_prefix(
        &self,
        prefix: impl AsRef<str>,
    ) -> Result<Vec<Commit>, WorkspaceError> {
        let prefix = prefix.as_ref();
        self.validate_commit_prefix(prefix)?;
        let mut tips = Vec::new();
        if let Some(head) = self.head_commit() {
            tips.push(ObjectId::from_hex(head.as_bytes()).map_err(|error| {
                self.revision_error(prefix, &format!("HEAD commit ID is invalid: {error}"))
            })?);
        }
        for choice in self.revision_choices()? {
            tips.push(
                ObjectId::from_hex(choice.commit().as_str().as_bytes()).map_err(|error| {
                    self.revision_error(prefix, &format!("reference commit ID is invalid: {error}"))
                })?,
            );
        }
        tips.sort();
        tips.dedup();
        self.walk_commits(tips, 0, None, Some(prefix))
    }

    /// Find commits under `revision` whose object IDs start with `prefix`.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when either input is invalid or history
    /// cannot be read.
    pub fn commits_from_matching_prefix(
        &self,
        revision: impl AsRef<str>,
        prefix: impl AsRef<str>,
    ) -> Result<Vec<Commit>, WorkspaceError> {
        let prefix = prefix.as_ref();
        self.validate_commit_prefix(prefix)?;
        let commit = self.resolve_revision(revision.as_ref())?;
        let tip = ObjectId::from_hex(commit.hex().as_bytes()).map_err(|error| {
            self.revision_error(
                revision.as_ref(),
                &format!("resolved commit ID is invalid: {error}"),
            )
        })?;
        self.walk_commits([tip], 0, None, Some(prefix))
    }

    fn validate_commit_prefix(&self, prefix: &str) -> Result<(), WorkspaceError> {
        if prefix.is_empty()
            || prefix.len() > 40
            || !prefix.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(
                self.revision_error(prefix, "commit prefix must be 1-40 hexadecimal digits")
            );
        }
        Ok(())
    }

    fn walk_commits(
        &self,
        tips: impl IntoIterator<Item = ObjectId>,
        offset: usize,
        limit: Option<usize>,
        prefix: Option<&str>,
    ) -> Result<Vec<Commit>, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Ok(Vec::new());
        };
        let tips: Vec<_> = tips.into_iter().collect();
        if tips.is_empty() {
            return Ok(Vec::new());
        }
        let walk = git
            .repo
            .rev_walk(tips)
            .sorting(Sorting::ByCommitTime(CommitTimeOrder::NewestFirst))
            .all()
            .map_err(|error| WorkspaceError {
                path: self.root.clone(),
                message: format!("cannot discover commit history: {error}"),
            })?;
        let mut commits = limit.map_or_else(Vec::new, Vec::with_capacity);
        let mut seen = 0;
        for info in walk {
            let info = info.map_err(|error| WorkspaceError {
                path: self.root.clone(),
                message: format!("cannot read commit history: {error}"),
            })?;
            let hex = info.id.to_hex().to_string();
            if prefix.is_some_and(|prefix| !hex.starts_with(prefix)) {
                continue;
            }
            if seen < offset {
                seen += 1;
                continue;
            }
            if limit.is_some_and(|limit| commits.len() >= limit) {
                break;
            }
            commits.push(
                commit_record(&git.repo, info.id).map_err(|message| WorkspaceError {
                    path: self.root.clone(),
                    message: format!("cannot read commit {hex}: {message}"),
                })?,
            );
        }
        Ok(commits)
    }

    /// Compare two repository-wide endpoints directly.
    ///
    /// Commit endpoints read their pinned trees, `Index` reads the current
    /// index, and `WorkingTree` reads the final eligible on-disk state.
    /// Staged and unstaged layers therefore cancel when their final working
    /// tree content equals the selected base.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when an endpoint cannot be enumerated.
    /// Missing blobs and unreadable mutable files remain explicit
    /// [`PathState::Missing`] changes rather than falling back to `HEAD`.
    pub fn compare(
        &mut self,
        base: ComparisonEndpoint,
        target: ComparisonEndpoint,
    ) -> Result<Comparison, WorkspaceError> {
        self.compare_with_manifest(base, target, None)
    }

    /// Compare endpoints while using one previously captured index snapshot.
    ///
    /// Every Index enumeration and read in this comparison uses `manifest`,
    /// even if the live index changes while the comparison is running.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the manifest belongs to another
    /// checkout or an endpoint cannot be read within the configured limits.
    pub fn compare_with_index_manifest(
        &mut self,
        base: ComparisonEndpoint,
        target: ComparisonEndpoint,
        manifest: &IndexManifest,
    ) -> Result<Comparison, WorkspaceError> {
        self.compare_with_manifest(base, target, Some(manifest))
    }

    fn compare_with_manifest(
        &mut self,
        base: ComparisonEndpoint,
        target: ComparisonEndpoint,
        manifest: Option<&IndexManifest>,
    ) -> Result<Comparison, WorkspaceError> {
        self.begin_comparison();
        let endpoint_files = |workspace: &mut Self,
                              endpoint: &ComparisonEndpoint|
         -> Result<BTreeMap<PathBuf, EndpointFile>, WorkspaceError> {
            if endpoint == &ComparisonEndpoint::Index
                && let Some(manifest) = manifest
            {
                return workspace.index_manifest_files(manifest);
            }
            workspace.endpoint_files(endpoint)
        };
        let base_files = endpoint_files(self, &base)?;
        let target_files = endpoint_files(self, &target)?;
        let target_paths = target_files
            .iter()
            .filter(|(_, file)| !file.info.mode().is_directory())
            .map(|(path, _)| path.clone())
            .collect();
        let mut paths = BTreeSet::new();
        paths.extend(base_files.keys().cloned());
        paths.extend(target_files.keys().cloned());
        self.check_path_count(paths.len())?;
        let mut changes = Vec::new();
        for path in paths {
            self.check_scan()?;
            let base_file = base_files.get(&path);
            let target_file = target_files.get(&path);
            let mut base_state = base_file.map_or(PathState::Absent, |file| {
                PathState::Present(file.info.clone())
            });
            let mut target_state = target_file.map_or(PathState::Absent, |file| {
                PathState::Present(file.info.clone())
            });
            let base_bytes = if base_file.is_some_and(|file| file.info.is_supported()) {
                match self.endpoint_bytes_with_manifest(&base, &path, manifest) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        base_state = PathState::Missing(error.to_string());
                        None
                    }
                }
            } else {
                None
            };
            let target_bytes = if target_file.is_some_and(|file| file.info.is_supported()) {
                match self.endpoint_bytes_with_manifest(&target, &path, manifest) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        target_state = PathState::Missing(error.to_string());
                        None
                    }
                }
            } else {
                None
            };
            if let Some(bytes) = base_bytes.as_deref()
                && let PathState::Present(info) = &mut base_state
            {
                *info = info
                    .clone()
                    .with_binary(self.is_binary(path.as_path(), bytes));
            }
            if let Some(bytes) = target_bytes.as_deref()
                && let PathState::Present(info) = &mut target_state
            {
                *info = info
                    .clone()
                    .with_binary(self.is_binary(path.as_path(), bytes));
            }
            if let Some(change) = PathChange::from_states(
                path,
                base_state,
                target_state,
                base_bytes.as_deref(),
                target_bytes.as_deref(),
            ) {
                changes.push(change);
            }
        }
        Ok(Comparison::from_parts(base, target, target_paths, changes))
    }

    fn endpoint_bytes_with_manifest(
        &self,
        endpoint: &ComparisonEndpoint,
        relative: &Path,
        manifest: Option<&IndexManifest>,
    ) -> Result<Option<Vec<u8>>, WorkspaceError> {
        if endpoint == &ComparisonEndpoint::Index
            && let Some(manifest) = manifest
        {
            return self.index_manifest_bytes(manifest, relative, self.content_limit());
        }
        self.endpoint_bytes(endpoint, relative)
    }

    /// Enumerate every non-directory path present at one endpoint.
    ///
    /// This operation evaluates only `endpoint`; it does not require a Base
    /// or validate a comparison pair. Review points are invalid Targets and
    /// must be resolved by their owning store instead.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the selected endpoint cannot be
    /// enumerated, including an unavailable commit or a review point.
    pub fn endpoint_paths(
        &mut self,
        endpoint: &ComparisonEndpoint,
    ) -> Result<Vec<PathBuf>, WorkspaceError> {
        Ok(self
            .endpoint_files(endpoint)?
            .into_iter()
            .filter_map(|(path, file)| (!file.info.mode().is_directory()).then_some(path))
            .collect())
    }

    /// Report facts for one path at a selected endpoint without reading its
    /// content.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the selected endpoint cannot be
    /// enumerated.
    pub fn endpoint_path_info(
        &mut self,
        endpoint: &ComparisonEndpoint,
        relative: &Path,
    ) -> Result<Option<PathInfo>, WorkspaceError> {
        self.check_scan()?;
        let failure = |message: String| WorkspaceError {
            path: self.root.join(relative),
            message,
        };
        let (mode, size, object) = match endpoint {
            ComparisonEndpoint::EmptyTree => return Ok(None),
            ComparisonEndpoint::WorkingTree => {
                let metadata = match fs::symlink_metadata(self.root.join(relative)) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => {
                        return Err(failure(format!(
                            "cannot inspect working-tree path: {error}"
                        )));
                    }
                };
                (
                    working_tree_mode(&metadata),
                    metadata.is_file().then_some(metadata.len()),
                    None,
                )
            }
            ComparisonEndpoint::Commit(id) => {
                let git = self.ignore.as_ref().ok_or_else(|| {
                    failure("commit paths are unavailable outside a Git repository".to_owned())
                })?;
                let id = ObjectId::from_hex(id.as_str().as_bytes())
                    .map_err(|error| failure(error.to_string()))?;
                let commit = git
                    .repo
                    .find_commit(id)
                    .map_err(|error| failure(format!("cannot read commit: {error}")))?;
                let tree = commit
                    .tree()
                    .map_err(|error| failure(format!("cannot read commit tree: {error}")))?;
                let Some(entry) = tree
                    .lookup_entry_by_path(relative)
                    .map_err(|error| failure(error.to_string()))?
                else {
                    return Ok(None);
                };
                let mode = tree_mode(entry.mode());
                let size = if mode.is_supported() {
                    Some(
                        git.repo
                            .find_header(entry.id())
                            .map_err(|error| failure(error.to_string()))?
                            .size(),
                    )
                } else {
                    None
                };
                (mode, size, Some(entry.id().to_hex().to_string()))
            }
            ComparisonEndpoint::Index => {
                let git = self.ignore.as_ref().ok_or_else(|| {
                    failure("index paths are unavailable outside a Git repository".to_owned())
                })?;
                let index = git
                    .repo
                    .index_or_empty()
                    .map_err(|error| failure(error.to_string()))?;
                let path = unix_path(relative);
                let Some(entry) = index.entry_by_path(path.as_ref()) else {
                    return Ok(None);
                };
                let mode = index_mode(entry.mode);
                let size = if mode.is_supported() {
                    Some(
                        git.repo
                            .find_header(entry.id)
                            .map_err(|error| failure(error.to_string()))?
                            .size(),
                    )
                } else {
                    None
                };
                (mode, size, Some(entry.id.to_hex().to_string()))
            }
            ComparisonEndpoint::ReviewPoint(id) => {
                return Err(failure(format!(
                    "review point {id} requires ReviewPointStore"
                )));
            }
        };
        Ok(Some(
            PathInfo::new(mode, size, object, false)
                .with_supported(endpoint_size_supported(mode, size)),
        ))
    }

    /// Load endpoint bytes for one repository-relative path.
    ///
    /// `None` means the endpoint has no path. It never means "use `HEAD`".
    /// A directory, submodule, unreadable file, or missing Git object is an
    /// error with the selected endpoint named in its message.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the endpoint is unavailable or its
    /// path cannot be read.
    pub fn endpoint_bytes(
        &self,
        endpoint: &ComparisonEndpoint,
        relative: &Path,
    ) -> Result<Option<Vec<u8>>, WorkspaceError> {
        self.check_scan()?;
        let bytes = match endpoint {
            ComparisonEndpoint::EmptyTree => Ok(None),
            ComparisonEndpoint::Commit(id) => self.commit_bytes(id, relative),
            ComparisonEndpoint::Index => self.index_endpoint_bytes(relative),
            ComparisonEndpoint::WorkingTree => self.working_tree_endpoint_bytes(relative),
            ComparisonEndpoint::ReviewPoint(id) => Err(WorkspaceError {
                path: self.root.join(relative),
                message: format!(
                    "review point {id} requires ReviewPointStore to load endpoint content"
                ),
            }),
        }?;
        if let Some(bytes) = &bytes {
            self.charge_content(bytes.len())?;
        }
        Ok(bytes)
    }

    /// Load endpoint UTF-8 text for one repository-relative path.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the endpoint bytes are unavailable or
    /// are not valid UTF-8.
    pub fn endpoint_text(
        &self,
        endpoint: &ComparisonEndpoint,
        relative: &Path,
    ) -> Result<Option<String>, WorkspaceError> {
        let Some(bytes) = self.endpoint_bytes(endpoint, relative)? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| WorkspaceError {
                path: self.root.join(relative),
                message: format!("{endpoint:?} content is not UTF-8 text: {error}"),
            })
    }

    fn commit_blob_source(
        &mut self,
        id: &CommitId,
        relative: &Path,
        max_object_bytes: u64,
    ) -> Result<Option<CommitBlobSource>, WorkspaceError> {
        self.check_scan()?;
        let failure = |message: String| WorkspaceError {
            path: self.root.join(relative),
            message: format!(
                "commit {} path {}: {message}",
                id.short(),
                relative.display()
            ),
        };
        let path = unix_path(relative);
        if relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
            || path.contains(&0)
            || path
                .split(|byte| *byte == b'/')
                .any(|part| part.is_empty() || part == b"." || part == b"..")
        {
            return Err(failure("invalid repository-relative path".to_owned()));
        }
        let repo =
            self.exact_repository(id.as_str(), max_object_bytes.max(MAX_EXACT_METADATA_BYTES))?;
        let object_id = ObjectId::from_hex(id.as_str().as_bytes())
            .map_err(|error| failure(format!("invalid commit object ID: {error}")))?;
        check_commit_header(&repo, object_id).map_err(failure)?;
        let commit = repo
            .find_commit(object_id)
            .map_err(|error| failure(format!("cannot read commit object: {error}")))?;
        let mut object_id = commit
            .tree_id()
            .map_err(|error| failure(format!("cannot read commit tree ID: {error}")))?
            .detach();
        drop(commit);
        let mut mode = FileMode::Directory;
        let mut components = path.split(|byte| *byte == b'/').peekable();
        while let Some(component) = components.next() {
            self.check_scan()?;
            let header = repo
                .find_header(object_id)
                .map_err(|error| failure(format!("cannot inspect commit tree: {error}")))?;
            if header.kind() != gix::objs::Kind::Tree {
                return Err(failure(
                    "directory entry names a non-tree object".to_owned(),
                ));
            }
            check_exact_metadata_size(header.size(), "commit tree").map_err(failure)?;
            let tree = repo
                .find_tree(object_id)
                .map_err(|error| failure(format!("cannot read commit tree: {error}")))?;
            let mut found = None;
            for entry in tree.iter() {
                self.check_scan()?;
                let entry =
                    entry.map_err(|error| failure(format!("cannot read tree entry: {error}")))?;
                if entry.filename() == component.as_bstr() {
                    found = Some((tree_mode(entry.mode()), entry.id().detach()));
                    break;
                }
            }
            let Some(entry) = found else {
                return Ok(None);
            };
            (mode, object_id) = entry;
            if components.peek().is_some() && mode != FileMode::Directory {
                return Err(failure(format!("unsupported {mode:?} intermediate entry")));
            }
        }
        if !matches!(mode, FileMode::Regular | FileMode::Executable) {
            return Err(failure(format!("unsupported {mode:?} entry")));
        }
        let header = repo
            .find_header(object_id)
            .map_err(|error| failure(format!("cannot inspect commit blob: {error}")))?;
        if header.kind() != gix::objs::Kind::Blob {
            return Err(failure("entry names a non-blob object".to_owned()));
        }
        Ok(Some(CommitBlobSource {
            repo,
            mode,
            size: header.size(),
            object: object_id,
        }))
    }

    fn load_commit_blob(
        &mut self,
        id: &CommitId,
        relative: &Path,
        source: &CommitBlobSource,
    ) -> Result<CommitBlob, WorkspaceError> {
        self.check_scan()?;
        let failure = |message: String| WorkspaceError {
            path: self.root.join(relative),
            message: format!(
                "commit {} path {}: {message}",
                id.short(),
                relative.display()
            ),
        };
        let blob = source
            .repo
            .find_blob(source.object)
            .map_err(|error| failure(format!("cannot read commit blob: {error}")))?;
        let bytes = blob.detach().data;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != source.size {
            return Err(failure("blob header size does not match data".to_owned()));
        }
        Ok(CommitBlob {
            mode: source.mode,
            size: source.size,
            object: source.object.to_hex().to_string(),
            bytes,
        })
    }

    /// Load one bounded regular-file blob from an exact commit tree.
    ///
    /// `None` means the path is absent. Regular and executable blobs are
    /// accepted; directories, symbolic links, submodules, unsupported modes,
    /// and entries naming non-blob objects are rejected. Intermediate entries
    /// must have directory mode and name actual trees before their data is
    /// loaded. Commit and tree metadata are independently bounded before
    /// decoding. Git replacements are ignored throughout. The blob header is
    /// checked against `max_bytes` before allocating blob data.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the commit, tree, path metadata, or
    /// blob is unavailable or corrupt, the entry is not a regular file, or
    /// its bytes exceed `max_bytes`. Paths must be nonempty, repository-relative,
    /// and contain no NUL, empty, `.` or `..` components.
    pub fn commit_blob(
        &mut self,
        id: &CommitId,
        relative: &Path,
        max_bytes: u64,
    ) -> Result<Option<CommitBlob>, WorkspaceError> {
        let Some(source) = self.commit_blob_source(id, relative, max_bytes)? else {
            return Ok(None);
        };
        if source.size > max_bytes {
            return Err(WorkspaceError {
                path: self.root.join(relative),
                message: format!(
                    "commit {} path {}: blob exceeds the {max_bytes}-byte limit",
                    id.short(),
                    relative.display()
                ),
            });
        }
        self.load_commit_blob(id, relative, &source).map(Some)
    }

    /// Verify named regular commit blobs against full-file identities.
    ///
    /// Distinct paths, including failures, are read at most once. Candidate
    /// count and loaded bytes share the configured comparison budgets. A file
    /// larger than the total byte budget is reported as unavailable so later
    /// candidates can progress. A file that fits the total budget but not the
    /// remaining aggregate budget returns typed continuation before loading.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the commit itself is unavailable or
    /// the candidate or path budget is exceeded.
    pub fn match_commit_files(
        &mut self,
        id: &CommitId,
        candidates: &[ExactFile],
    ) -> Result<ExactFileBatch, WorkspaceError> {
        self.check_path_count(candidates.len())?;
        let checkout = self.identity();
        if candidates
            .iter()
            .any(|candidate| candidate.checkout() != &checkout)
        {
            return Err(
                self.scan_error("exact-file candidate belongs to another checkout or repository")
            );
        }
        self.commit(id)?;
        self.begin_comparison();
        let total_limit = self.limits.comparison_bytes;
        let mut cache = BTreeMap::<PathBuf, Result<Option<FullFileDigest>, String>>::new();
        let mut results = Vec::with_capacity(candidates.len());
        for (index, candidate) in candidates.iter().enumerate() {
            if let Some(result) = cache.get(candidate.path()) {
                results.push(exact_file_result(candidate, result));
                continue;
            }
            self.check_path_count(cache.len().saturating_add(1))?;
            let source = match self.commit_blob_source(id, candidate.path(), total_limit) {
                Ok(source) => source,
                Err(error) => {
                    let loaded = Err(error.to_string());
                    results.push(exact_file_result(candidate, &loaded));
                    cache.insert(candidate.path.clone(), loaded);
                    continue;
                }
            };
            let Some(source) = source else {
                let loaded = Ok(None);
                results.push(exact_file_result(candidate, &loaded));
                cache.insert(candidate.path.clone(), loaded);
                continue;
            };
            if source.size > total_limit {
                let loaded = Err(format!(
                    "commit {} path {}: blob exceeds the {total_limit}-byte limit",
                    id.short(),
                    candidate.path.display()
                ));
                results.push(exact_file_result(candidate, &loaded));
                cache.insert(candidate.path.clone(), loaded);
                continue;
            }
            if source.size > self.content_limit() {
                return Ok(ExactFileBatch {
                    results,
                    progress: ExactFileProgress::ContinueAt { next: index },
                });
            }

            self.charge_content(usize::try_from(source.size).unwrap_or(usize::MAX))?;
            let result = self
                .load_commit_blob(id, candidate.path(), &source)
                .map(|blob| Some(FullFileDigest::from_bytes(blob.bytes())))
                .map_err(|error| error.to_string());
            results.push(exact_file_result(candidate, &result));
            cache.insert(candidate.path.clone(), result);
        }
        Ok(ExactFileBatch {
            results,
            progress: ExactFileProgress::Complete,
        })
    }

    /// Capture one complete immutable index manifest within comparison limits.
    ///
    /// Split indexes are resolved by the index reader. Conflicts,
    /// intent-to-add entries, sparse directories, invalid paths, and unknown
    /// modes return an explicit unavailable result rather than a partial
    /// manifest.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the index cannot be read or a finite
    /// path/manifest-byte limit is exceeded.
    pub fn index_manifest(&self) -> Result<IndexManifestCapture, WorkspaceError> {
        let index = self.bounded_index()?;
        self.check_path_count(index.entries().len())?;
        if index.is_sparse() {
            return Ok(IndexManifestCapture::Unavailable(
                IndexManifestUnavailable::Sparse,
            ));
        }
        let mut entries = Vec::with_capacity(index.entries().len());
        let mut canonical_bytes = 8_u64;
        for entry in index.entries() {
            self.check_scan()?;
            let path = entry.path(&index);
            if entry.stage() != gix::index::entry::Stage::Unconflicted {
                return Ok(IndexManifestCapture::Unavailable(
                    IndexManifestUnavailable::Conflict {
                        path: Box::from(path.as_ref()),
                    },
                ));
            }
            if entry
                .flags
                .contains(gix::index::entry::Flags::INTENT_TO_ADD)
            {
                return Ok(IndexManifestCapture::Unavailable(
                    IndexManifestUnavailable::IntentToAdd {
                        path: Box::from(path.as_ref()),
                    },
                ));
            }
            if !valid_git_path(path.as_ref()) {
                return Ok(IndexManifestCapture::Unavailable(
                    IndexManifestUnavailable::Unsupported {
                        path: Box::from(path.as_ref()),
                        detail: "invalid repository-relative path".to_owned(),
                    },
                ));
            }
            let mode = index_mode(entry.mode);
            if matches!(mode, FileMode::Directory | FileMode::Other(_)) {
                return Ok(IndexManifestCapture::Unavailable(
                    IndexManifestUnavailable::Unsupported {
                        path: Box::from(path.as_ref()),
                        detail: format!("unsupported index mode {mode:?}"),
                    },
                ));
            }
            canonical_bytes = canonical_bytes
                .saturating_add(8)
                .saturating_add(u64::try_from(path.len()).unwrap_or(u64::MAX))
                .saturating_add(4)
                .saturating_add(u64::try_from(entry.id.as_bytes().len()).unwrap_or(u64::MAX));
            if canonical_bytes > self.limits.comparison_bytes {
                return Err(self.scan_error(
                    "index manifest limited by comparison byte budget; coverage is incomplete",
                ));
            }
            entries.push(IndexManifestEntry {
                path: Box::from(path.as_ref()),
                mode,
                object: entry.id.to_hex().to_string(),
                assume_unchanged: entry.flags.contains(gix::index::entry::Flags::ASSUME_VALID),
                skip_worktree: entry
                    .flags
                    .contains(gix::index::entry::Flags::SKIP_WORKTREE),
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        if entries.windows(2).any(|pair| pair[0].path == pair[1].path) {
            return Ok(IndexManifestCapture::Unavailable(
                IndexManifestUnavailable::Unsupported {
                    path: Box::new([]),
                    detail: "duplicate index path".to_owned(),
                },
            ));
        }
        let identity = manifest_identity(&entries);
        Ok(IndexManifestCapture::Available(IndexManifest {
            checkout: self.identity(),
            identity,
            entries: entries.into_boxed_slice(),
        }))
    }

    fn bounded_index(&self) -> Result<gix::index::File, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Err(self.scan_error("index manifest is unavailable outside a Git repository"));
        };
        let index_exists = match fs::metadata(git.repo.index_path()) {
            Ok(metadata) if metadata.len() > self.limits.comparison_bytes => {
                return Err(self.scan_error(
                    "index file limited by comparison byte budget; coverage is incomplete",
                ));
            }
            Ok(_) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(self.scan_error(format!("cannot inspect the index: {error}")));
            }
        };
        let repo = gix::open_opts(
            git.repo.git_dir(),
            exact_open_options(self.limits.comparison_bytes).open_path_as_is(true),
        )
        .map_err(|error| self.scan_error(format!("cannot open the bounded index: {error}")))?;
        if index_exists {
            repo.open_index().map_err(|error| {
                self.scan_error(format!("cannot read the index manifest: {error}"))
            })
        } else {
            Ok(gix::index::File::from_state(
                gix::index::State::new(repo.object_hash()),
                repo.index_path(),
            ))
        }
    }

    /// Read bytes for one path from a captured index manifest.
    ///
    /// The captured object ID is used even if the live index changes. No
    /// checkout filters are executed.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] for a foreign manifest, invalid path,
    /// unsupported mode, missing/non-blob object, or an exceeded byte limit.
    pub fn index_manifest_bytes(
        &self,
        manifest: &IndexManifest,
        relative: &Path,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, WorkspaceError> {
        if manifest.checkout != self.identity() {
            return Err(self.scan_error("index manifest belongs to another checkout"));
        }
        let path = unix_path(relative);
        if !valid_git_path(path.as_ref()) {
            return Err(self.scan_error("invalid index manifest path"));
        }
        let Some(entry) = manifest.entry(path.as_ref()) else {
            return Ok(None);
        };
        if !matches!(
            entry.mode,
            FileMode::Regular | FileMode::Executable | FileMode::Symlink
        ) {
            return Err(self.scan_error(format!(
                "index manifest path has unsupported {:?} mode",
                entry.mode
            )));
        }
        let git = self
            .ignore
            .as_ref()
            .ok_or_else(|| self.scan_error("index manifest content is unavailable outside Git"))?;
        let id = ObjectId::from_hex(entry.object.as_bytes())
            .map_err(|error| self.scan_error(format!("invalid manifest object ID: {error}")))?;
        let header = git
            .repo
            .find_header(id)
            .map_err(|error| self.scan_error(format!("cannot inspect manifest blob: {error}")))?;
        if header.kind() != gix::objs::Kind::Blob {
            return Err(self.scan_error("index manifest entry names a non-blob object"));
        }
        if header.size() > max_bytes {
            return Err(self.scan_error(format!(
                "index manifest blob exceeds the {max_bytes}-byte limit"
            )));
        }
        self.charge_content(usize::try_from(header.size()).map_err(|_error| {
            self.scan_error("index manifest blob size exceeds the platform limit")
        })?)?;
        let bytes = git
            .repo
            .find_blob(id)
            .map_err(|error| self.scan_error(format!("cannot read manifest blob: {error}")))?
            .detach()
            .data;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != header.size() {
            return Err(self.scan_error("manifest blob header size does not match data"));
        }
        Ok(Some(bytes))
    }

    /// Report captured Index facts for one repository-relative path.
    ///
    /// Gitlinks are metadata-only and are never dereferenced in the
    /// superproject object database.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] for a foreign manifest, invalid path, or
    /// malformed/missing blob metadata for a supported file mode.
    pub fn index_manifest_path_info(
        &self,
        manifest: &IndexManifest,
        relative: &Path,
    ) -> Result<Option<PathInfo>, WorkspaceError> {
        if manifest.checkout != self.identity() {
            return Err(self.scan_error("index manifest belongs to another checkout"));
        }
        let path = unix_path(relative);
        if !valid_git_path(path.as_ref()) {
            return Err(self.scan_error("invalid index manifest path"));
        }
        let Some(entry) = manifest.entry(path.as_ref()) else {
            return Ok(None);
        };
        let size = if entry.mode == FileMode::Submodule {
            None
        } else {
            let git = self.ignore.as_ref().ok_or_else(|| {
                self.scan_error("index manifest content is unavailable outside Git")
            })?;
            let id = ObjectId::from_hex(entry.object.as_bytes())
                .map_err(|error| self.scan_error(format!("invalid manifest object ID: {error}")))?;
            let header = git.repo.find_header(id).map_err(|error| {
                self.scan_error(format!("cannot inspect manifest blob: {error}"))
            })?;
            if matches!(
                entry.mode,
                FileMode::Regular | FileMode::Executable | FileMode::Symlink
            ) && header.kind() != gix::objs::Kind::Blob
            {
                return Err(self.scan_error("index manifest entry names a non-blob object"));
            }
            Some(header.size())
        };
        Ok(Some(
            PathInfo::new(entry.mode, size, Some(entry.object.clone()), false)
                .with_supported(endpoint_size_supported(entry.mode, size)),
        ))
    }

    /// Enumerate non-directory paths from one captured Index manifest.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] for a foreign manifest or invalid path.
    pub fn index_manifest_paths(
        &self,
        manifest: &IndexManifest,
    ) -> Result<Vec<PathBuf>, WorkspaceError> {
        if manifest.checkout != self.identity() {
            return Err(self.scan_error("index manifest belongs to another checkout"));
        }
        self.check_path_count(manifest.entries.len())?;
        let mut paths = Vec::with_capacity(manifest.entries.len());
        for entry in &manifest.entries {
            self.check_scan()?;
            if !valid_git_path(&entry.path) {
                return Err(self.scan_error("invalid index manifest path"));
            }
            paths.push(gix::path::from_bstr(entry.path.as_bstr()).into_owned());
        }
        Ok(paths)
    }

    /// Compare a captured index manifest with one exact commit tree.
    ///
    /// Equality covers raw path bytes, Git modes, and exact object IDs. It
    /// does not write trees or execute checkout filters.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] for a foreign manifest, unavailable commit
    /// tree, cancellation, or exceeded path/manifest-byte limits.
    pub fn index_manifest_matches_commit(
        &self,
        manifest: &IndexManifest,
        id: &CommitId,
    ) -> Result<bool, WorkspaceError> {
        if manifest.checkout.repository != self.key {
            return Err(self.scan_error("index manifest belongs to another repository"));
        }
        let entries = self.exact_commit_entries(id)?;
        Ok(entries.len() == manifest.entries.len()
            && entries
                .iter()
                .zip(manifest.entries.iter())
                .all(|(left, right)| {
                    left.path == right.path
                        && left.mode == right.mode
                        && left.object == right.object
                }))
    }

    fn exact_commit_entries(
        &self,
        id: &CommitId,
    ) -> Result<Vec<IndexManifestEntry>, WorkspaceError> {
        self.begin_comparison();
        let repo = self.exact_repository(id.as_str(), MAX_EXACT_METADATA_BYTES)?;
        let object_id = ObjectId::from_hex(id.as_str().as_bytes()).map_err(|error| {
            self.revision_error(id.as_str(), &format!("invalid commit object ID: {error}"))
        })?;
        check_commit_header(&repo, object_id)
            .map_err(|message| self.revision_error(id.as_str(), &message))?;
        let commit = repo.find_commit(object_id).map_err(|error| {
            self.revision_error(id.as_str(), &format!("cannot read commit object: {error}"))
        })?;
        let tree = commit
            .tree_id()
            .map_err(|error| {
                self.revision_error(id.as_str(), &format!("cannot read commit tree ID: {error}"))
            })?
            .detach();
        drop(commit);
        let traversal_limit = self.limits.retained_paths;
        let mut traversal_items = 0_usize;
        self.charge_tree_item(&mut traversal_items, traversal_limit)?;
        let mut pending = vec![(tree, Vec::<u8>::new())];
        let mut entries = Vec::new();
        while let Some((tree, prefix)) = pending.pop() {
            self.check_scan()?;
            let header = repo.find_header(tree).map_err(|error| {
                self.revision_error(id.as_str(), &format!("cannot inspect commit tree: {error}"))
            })?;
            if header.kind() != gix::objs::Kind::Tree {
                return Err(self.revision_error(
                    id.as_str(),
                    "commit directory entry names a non-tree object",
                ));
            }
            check_exact_metadata_size(header.size(), "commit tree")
                .map_err(|message| self.revision_error(id.as_str(), &message))?;
            self.charge_content(usize::try_from(header.size()).map_err(|_error| {
                self.scan_error("commit tree size exceeds the platform limit")
            })?)?;
            let tree = repo.find_tree(tree).map_err(|error| {
                self.revision_error(id.as_str(), &format!("cannot read commit tree: {error}"))
            })?;
            for entry in tree.iter() {
                self.check_scan()?;
                self.charge_tree_item(&mut traversal_items, traversal_limit)?;
                let entry = entry.map_err(|error| {
                    self.revision_error(id.as_str(), &format!("cannot read tree entry: {error}"))
                })?;
                let separator = usize::from(!prefix.is_empty());
                let path_len = prefix
                    .len()
                    .saturating_add(separator)
                    .saturating_add(entry.filename().len());
                let fixed_bytes = 8_usize
                    .saturating_add(4)
                    .saturating_add(entry.id().as_bytes().len());
                self.charge_content(path_len.saturating_add(fixed_bytes))?;
                let mut path = Vec::with_capacity(path_len);
                path.extend_from_slice(&prefix);
                if separator != 0 {
                    path.push(b'/');
                }
                path.extend_from_slice(entry.filename());
                if !valid_git_path(&path) {
                    return Err(self.revision_error(
                        id.as_str(),
                        "commit tree contains an invalid repository-relative path",
                    ));
                }
                let mode = tree_mode(entry.mode());
                if mode == FileMode::Directory {
                    self.charge_tree_item(&mut traversal_items, traversal_limit)?;
                    self.charge_content(path.len())?;
                    pending.push((entry.id().detach(), path));
                    continue;
                }
                self.check_path_count(entries.len().saturating_add(1))?;
                entries.push(IndexManifestEntry {
                    path: path.into_boxed_slice(),
                    mode,
                    object: entry.id().to_hex().to_string(),
                    assume_unchanged: false,
                    skip_worktree: false,
                });
            }
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    fn charge_tree_item(&self, used: &mut usize, limit: usize) -> Result<(), WorkspaceError> {
        *used = used.saturating_add(1);
        if *used > limit {
            return Err(self.scan_error(
                "commit tree traversal limited by item budget; coverage is incomplete",
            ));
        }
        Ok(())
    }

    pub(crate) fn endpoint_files(
        &mut self,
        endpoint: &ComparisonEndpoint,
    ) -> Result<BTreeMap<PathBuf, EndpointFile>, WorkspaceError> {
        match endpoint {
            ComparisonEndpoint::EmptyTree => Ok(BTreeMap::new()),
            ComparisonEndpoint::Commit(id) => self.commit_files(id),
            ComparisonEndpoint::Index => self.index_files(),
            ComparisonEndpoint::WorkingTree => self.working_tree_files(),
            ComparisonEndpoint::ReviewPoint(id) => Err(WorkspaceError {
                path: self.root.clone(),
                message: format!(
                    "review point {id} requires ReviewPointStore to enumerate endpoint paths"
                ),
            }),
        }
    }

    fn index_manifest_files(
        &mut self,
        manifest: &IndexManifest,
    ) -> Result<BTreeMap<PathBuf, EndpointFile>, WorkspaceError> {
        if manifest.checkout != self.identity() {
            return Err(self.scan_error("index manifest belongs to another checkout"));
        }
        self.check_path_count(manifest.entries.len())?;
        let mut files = BTreeMap::new();
        for entry in &manifest.entries {
            self.check_scan()?;
            let path = gix::path::from_bstr(entry.path.as_bstr()).into_owned();
            let info = self
                .index_manifest_path_info(manifest, &path)?
                .ok_or_else(|| self.scan_error("index manifest path disappeared"))?;
            files.insert(path, EndpointFile { info });
        }
        Ok(files)
    }

    fn revision_error(&self, revision: &str, detail: &str) -> WorkspaceError {
        WorkspaceError {
            path: self.root.clone(),
            message: format!("cannot resolve revision `{revision}`: {detail}"),
        }
    }

    fn resolve_revision_id(
        &self,
        repo: &gix::Repository,
        revision: &str,
    ) -> Result<ObjectId, WorkspaceError> {
        if let Some((base, ancestry)) = parent_expression(revision) {
            let base_id = self.resolve_revision_id(repo, base)?;
            match ancestry {
                Ancestry::FirstParents(count) => {
                    let mut id = base_id;
                    for _ in 0..count {
                        let commit = repo.find_commit(id).map_err(|error| {
                            self.revision_error(
                                revision,
                                &format!("cannot read parent of {}: {error}", id.to_hex()),
                            )
                        })?;
                        let Some(next) = commit.parent_ids().next() else {
                            return Err(self.revision_error(
                                revision,
                                &format!("{} has no requested parent", id.to_hex()),
                            ));
                        };
                        id = next.detach();
                    }
                    return Ok(id);
                }
                Ancestry::Parent(0) => return Ok(base_id),
                Ancestry::Parent(number) => {
                    let commit = repo.find_commit(base_id).map_err(|error| {
                        self.revision_error(
                            revision,
                            &format!("cannot read parent of {}: {error}", base_id.to_hex()),
                        )
                    })?;
                    let Some(parent) = commit.parent_ids().nth(number - 1) else {
                        return Err(self.revision_error(
                            revision,
                            &format!("{} has no parent number {number}", base_id.to_hex()),
                        ));
                    };
                    return Ok(parent.detach());
                }
            }
        }

        if let Ok(mut reference) = repo.find_reference(revision) {
            let commit = reference.peel_to_commit().map_err(|error| {
                self.revision_error(revision, &format!("not a commit: {error}"))
            })?;
            return Ok(commit.id);
        }

        if revision.len() < 40
            && let Ok(prefix) = gix::hash::Prefix::from_hex(revision)
        {
            match repo.objects.lookup_prefix(prefix, None).map_err(|error| {
                self.revision_error(revision, &format!("cannot search object IDs: {error}"))
            })? {
                Some(Ok(id)) => {
                    let object = repo.find_object(id).map_err(|error| {
                        self.revision_error(revision, &format!("object is unavailable: {error}"))
                    })?;
                    let commit = object.peel_to_commit().map_err(|error| {
                        self.revision_error(revision, &format!("object is not a commit: {error}"))
                    })?;
                    return Ok(commit.id);
                }
                Some(Err(())) => {
                    return Err(self.revision_error(
                        revision,
                        "abbreviated object ID is ambiguous; use a longer ID",
                    ));
                }
                None => {}
            }
        }

        let id = ObjectId::from_hex(revision.as_bytes()).map_err(|error| {
            self.revision_error(
                revision,
                &format!("no local branch or tag matches, and it is not an object ID: {error}"),
            )
        })?;
        let object = repo.find_object(id).map_err(|error| {
            self.revision_error(revision, &format!("object is unavailable: {error}"))
        })?;
        let commit = object.peel_to_commit().map_err(|error| {
            self.revision_error(revision, &format!("object is not a commit: {error}"))
        })?;
        Ok(commit.id)
    }

    fn commit_bytes(
        &self,
        id: &CommitId,
        relative: &Path,
    ) -> Result<Option<Vec<u8>>, WorkspaceError> {
        self.commit_bytes_bounded(id, relative, self.content_limit())
    }

    fn commit_bytes_bounded(
        &self,
        id: &CommitId,
        relative: &Path,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Err(self.revision_error(
                id.as_str(),
                "commit content is unavailable outside a Git repository",
            ));
        };
        let object_id = ObjectId::from_hex(id.as_str().as_bytes()).map_err(|error| {
            self.revision_error(id.as_str(), &format!("invalid commit object ID: {error}"))
        })?;
        let commit = git
            .repo
            .find_commit(object_id)
            .map_err(|error| WorkspaceError {
                path: self.root.join(relative),
                message: format!("cannot read commit {}: {error}", id.short()),
            })?;
        let tree = commit.tree().map_err(|error| WorkspaceError {
            path: self.root.join(relative),
            message: format!("cannot read tree of commit {}: {error}", id.short()),
        })?;
        let Some(entry) = tree
            .lookup_entry_by_path(relative)
            .map_err(|error| WorkspaceError {
                path: self.root.join(relative),
                message: format!(
                    "cannot look up {} in commit {}: {error}",
                    relative.display(),
                    id.short()
                ),
            })?
        else {
            return Ok(None);
        };
        if !entry.mode().is_blob_or_symlink() {
            return Err(WorkspaceError {
                path: self.root.join(relative),
                message: format!(
                    "commit {} has an unsupported non-file entry at {}",
                    id.short(),
                    relative.display()
                ),
            });
        }
        let header = git
            .repo
            .find_header(entry.id())
            .map_err(|error| WorkspaceError {
                path: self.root.join(relative),
                message: format!("cannot inspect commit blob: {error}"),
            })?;
        if header.kind() != gix::objs::Kind::Blob {
            return Err(WorkspaceError {
                path: self.root.join(relative),
                message: format!(
                    "commit {} entry at {} names a non-blob object",
                    id.short(),
                    relative.display()
                ),
            });
        }
        if header.size() > max_bytes {
            return Err(WorkspaceError {
                path: self.root.join(relative),
                message: format!(
                    "commit {} blob at {} exceeds the {max_bytes}-byte limit",
                    id.short(),
                    relative.display()
                ),
            });
        }
        let object = entry.object().map_err(|error| WorkspaceError {
            path: self.root.join(relative),
            message: format!(
                "cannot read blob for {} at commit {}: {error}",
                relative.display(),
                id.short()
            ),
        })?;
        if object.kind != gix::objs::Kind::Blob {
            return Err(WorkspaceError {
                path: self.root.join(relative),
                message: format!(
                    "commit {} entry at {} is not a blob",
                    id.short(),
                    relative.display()
                ),
            });
        }
        let bytes = object.detach().data;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
            return Err(WorkspaceError {
                path: self.root.join(relative),
                message: format!(
                    "commit {} blob at {} exceeds the {max_bytes}-byte limit",
                    id.short(),
                    relative.display()
                ),
            });
        }
        Ok(Some(bytes))
    }

    fn index_endpoint_bytes(&self, relative: &Path) -> Result<Option<Vec<u8>>, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Err(WorkspaceError {
                path: self.root.join(relative),
                message: "index content is unavailable outside a Git repository".to_owned(),
            });
        };
        let index = git.repo.index_or_empty().map_err(|error| WorkspaceError {
            path: self.root.join(relative),
            message: format!("cannot read the index: {error}"),
        })?;
        let path = unix_path(relative);
        let Some(entry) = index.entry_by_path(path.as_ref()) else {
            return Ok(None);
        };
        if !is_status_entry(entry) {
            return Err(WorkspaceError {
                path: self.root.join(relative),
                message: format!("index has an unsupported entry at {}", relative.display()),
            });
        }
        let size = git
            .repo
            .find_header(entry.id)
            .map_err(|error| WorkspaceError {
                path: self.root.join(relative),
                message: format!("cannot inspect index blob: {error}"),
            })?
            .size();
        if size > self.content_limit() {
            return Err(self
                .scan_error("comparison limited by content byte budget; coverage is incomplete"));
        }
        let object = git
            .repo
            .find_object(entry.id)
            .map_err(|error| WorkspaceError {
                path: self.root.join(relative),
                message: format!("cannot read index blob at {}: {error}", relative.display()),
            })?;
        Ok(Some(object.detach().data))
    }

    fn working_tree_endpoint_bytes(
        &self,
        relative: &Path,
    ) -> Result<Option<Vec<u8>>, WorkspaceError> {
        let absolute = self.root.join(relative);
        let metadata = match fs::symlink_metadata(&absolute) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(WorkspaceError {
                    path: absolute,
                    message: format!("cannot inspect working-tree path: {error}"),
                });
            }
        };
        if metadata.is_symlink() {
            return fs::read_link(&absolute)
                .map(|target| Some(target.to_string_lossy().into_owned().into_bytes()))
                .map_err(|error| WorkspaceError {
                    path: absolute,
                    message: format!("cannot read symbolic link: {error}"),
                });
        }
        if metadata.is_file() {
            let limit = self.content_limit();
            if metadata.len() > limit {
                return Err(self.scan_error(
                    "comparison limited by content byte budget; coverage is incomplete",
                ));
            }
            let mut bytes = Vec::new();
            fs::File::open(&absolute)
                .and_then(|file| file.take(limit.saturating_add(1)).read_to_end(&mut bytes))
                .map_err(|error| WorkspaceError {
                    path: absolute,
                    message: format!("cannot read working-tree file: {error}"),
                })?;
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
                return Err(self.scan_error(
                    "comparison limited by content byte budget; coverage is incomplete",
                ));
            }
            return Ok(Some(bytes));
        }
        Err(WorkspaceError {
            path: absolute,
            message: "working-tree path is not a regular file or symbolic link".to_owned(),
        })
    }

    fn commit_files(
        &self,
        id: &CommitId,
    ) -> Result<BTreeMap<PathBuf, EndpointFile>, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Err(self.revision_error(
                id.as_str(),
                "commit paths are unavailable outside a Git repository",
            ));
        };
        let object_id = ObjectId::from_hex(id.as_str().as_bytes()).map_err(|error| {
            self.revision_error(id.as_str(), &format!("invalid commit object ID: {error}"))
        })?;
        let commit = git
            .repo
            .find_commit(object_id)
            .map_err(|error| WorkspaceError {
                path: self.root.clone(),
                message: format!("cannot read commit {}: {error}", id.short()),
            })?;
        let tree = commit.tree().map_err(|error| WorkspaceError {
            path: self.root.clone(),
            message: format!("cannot read tree of commit {}: {error}", id.short()),
        })?;
        let mut files = BTreeMap::new();
        collect_endpoint_tree(self, &git.repo, &tree, &mut files).map_err(|message| {
            WorkspaceError {
                path: self.root.clone(),
                message: format!("cannot enumerate commit {}: {message}", id.short()),
            }
        })?;
        Ok(files)
    }

    fn index_files(&self) -> Result<BTreeMap<PathBuf, EndpointFile>, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Err(WorkspaceError {
                path: self.root.clone(),
                message: "index paths are unavailable outside a Git repository".to_owned(),
            });
        };
        let index = git.repo.index_or_empty().map_err(|error| WorkspaceError {
            path: self.root.clone(),
            message: format!("cannot read the index: {error}"),
        })?;
        let mut files = BTreeMap::new();
        for entry in index.entries() {
            self.check_path_count(files.len().saturating_add(1))?;
            if entry.stage() != gix::index::entry::Stage::Unconflicted {
                continue;
            }
            let path = gix::path::from_bstr(entry.path(&index)).into_owned();
            let mode = index_mode(entry.mode);
            let size = git
                .repo
                .find_header(entry.id)
                .ok()
                .map(|header| header.size());
            files.insert(
                path,
                EndpointFile {
                    info: PathInfo::new(mode, size, Some(entry.id.to_hex().to_string()), false)
                        .with_supported(endpoint_size_supported(mode, size)),
                },
            );
        }
        Ok(files)
    }

    fn working_tree_files(&mut self) -> Result<BTreeMap<PathBuf, EndpointFile>, WorkspaceError> {
        let mut paths: BTreeSet<PathBuf> = self
            .walk_files_checked(Filter::Visible)?
            .into_iter()
            .collect();
        if let Some(git) = self.ignore.as_ref() {
            // Tracked ignored paths stay eligible without descending ignored output.
            let index = git.repo.index_or_empty().map_err(|error| WorkspaceError {
                path: self.root.clone(),
                message: format!("cannot read index paths for working-tree endpoint: {error}"),
            })?;
            for entry in index.entries() {
                paths.insert(gix::path::from_bstr(entry.path(&index)).into_owned());
                self.check_path_count(paths.len())?;
            }
        }
        let mut files = BTreeMap::new();
        for relative in paths {
            self.check_scan()?;
            let absolute = self.root.join(&relative);
            let metadata = match fs::symlink_metadata(&absolute) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(WorkspaceError {
                        path: absolute,
                        message: format!("cannot inspect working-tree path: {error}"),
                    });
                }
            };
            let mode = working_tree_mode(&metadata);
            let size = metadata.is_file().then_some(metadata.len());
            let mut parent = relative.parent();
            while let Some(directory) = parent.filter(|path| !path.as_os_str().is_empty()) {
                let absolute_directory = self.root.join(directory);
                let is_directory = fs::symlink_metadata(&absolute_directory)
                    .is_ok_and(|metadata| metadata.is_dir() && !metadata.is_symlink());
                if is_directory {
                    files
                        .entry(directory.to_path_buf())
                        .or_insert_with(|| EndpointFile {
                            info: PathInfo::new(FileMode::Directory, None, None, false),
                        });
                }
                parent = directory.parent();
                self.check_path_count(files.len())?;
            }
            files.insert(
                relative,
                EndpointFile {
                    info: PathInfo::new(mode, size, None, false)
                        .with_supported(endpoint_size_supported(mode, size)),
                },
            );
            self.check_path_count(files.len())?;
        }
        Ok(files)
    }

    fn is_binary(&mut self, relative: &Path, bytes: &[u8]) -> bool {
        self.diff_attr(relative).classify(bytes).unwrap_or(false)
    }

    /// Which of `wanted` (hex commits) are `HEAD` or one of its ancestors
    /// (ADR 0024). One walk from `HEAD` answers the whole set; it stops as
    /// soon as every wanted commit has been met, and never descends past
    /// the committer time of the oldest one, less a week of slack: a
    /// commit a rewrite dropped costs the history since it was made, not
    /// the whole history. A commit the object store no longer holds is
    /// unreachable without a walk. Returns `None` outside git or before
    /// the first commit, meaning nothing can be scoped.
    #[must_use]
    pub fn reachable<'a>(
        &self,
        wanted: impl IntoIterator<Item = &'a str>,
    ) -> Option<HashSet<String>> {
        let git = self.ignore.as_ref()?;
        let head = git.repo.head_id().ok()?;
        let mut pending: HashSet<String> = wanted.into_iter().map(str::to_owned).collect();
        let mut found = HashSet::new();
        if pending.is_empty() {
            return Some(found);
        }
        // A commit whose object is gone can be reached by nothing, and
        // the committer time of the rest bounds the walk: `HEAD`'s history
        // below the oldest wanted commit cannot hold one of them.
        let mut oldest = gix::date::SecondsSinceUnixEpoch::MAX;
        pending.retain(|hex| match commit_time(&git.repo, hex) {
            Some(seconds) => {
                oldest = oldest.min(seconds);
                true
            }
            None => false,
        });
        if pending.is_empty() {
            return Some(found);
        }
        let sorting = Sorting::ByCommitTimeCutoff {
            order: CommitTimeOrder::NewestFirst,
            seconds: oldest.saturating_sub(REACH_SLACK),
        };
        let walk = match git.repo.rev_walk([head]).sorting(sorting).all() {
            Ok(walk) => walk,
            Err(error) => {
                tracing::warn!(%error, "cannot walk history from HEAD");
                return Some(found);
            }
        };
        for info in walk {
            let info = match info {
                Ok(info) => info,
                Err(error) => {
                    tracing::warn!(%error, "history walk stopped early");
                    break;
                }
            };
            let hex = info.id.to_hex().to_string();
            if pending.remove(&hex) {
                found.insert(hex);
                if pending.is_empty() {
                    break;
                }
            }
        }
        Some(found)
    }

    /// The text of root-relative `relative` as committed at `HEAD`, the
    /// diff source for the gutter and the diff view (ADR 0006).
    ///
    /// Returns `None` outside git, and `Some("")` for a file `HEAD` does
    /// not have (unborn branch, untracked, or newly added), so every line
    /// of it counts as added.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when `HEAD` or the blob cannot be read,
    /// or the blob is not UTF-8 text.
    pub fn head_text(&self, relative: &Path) -> Result<Option<String>, WorkspaceError> {
        let Some(bytes) = self.head_bytes(relative)? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| WorkspaceError {
                path: self.root.join(relative),
                message: format!("HEAD blob is not UTF-8 text: {error}"),
            })
    }

    /// The text of `relative` at `HEAD` when its blob fits `max_bytes`.
    ///
    /// Returns `None` outside Git or when the blob exceeds the limit. An
    /// unborn `HEAD` or absent path is an empty string. The object header is
    /// checked before the immutable blob is loaded.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when `HEAD`, its tree, or the blob cannot be
    /// read, or when the blob is not UTF-8 text.
    pub fn head_text_bounded(
        &self,
        relative: &Path,
        max_bytes: u64,
    ) -> Result<Option<String>, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Ok(None);
        };
        let fail = |message: String| WorkspaceError {
            path: self.root.join(relative),
            message,
        };
        if git
            .repo
            .head()
            .map_err(|error| fail(format!("cannot read HEAD: {error}")))?
            .is_unborn()
        {
            return Ok(Some(String::new()));
        }
        let tree = git
            .repo
            .head_tree()
            .map_err(|error| fail(format!("cannot read HEAD tree: {error}")))?;
        let Some(entry) = tree.lookup_entry_by_path(relative).map_err(|error| {
            fail(format!(
                "cannot look up {} in HEAD: {error}",
                relative.display()
            ))
        })?
        else {
            return Ok(Some(String::new()));
        };
        if !entry.mode().is_blob_or_symlink() {
            return Err(fail(
                "HEAD has a directory or submodule at this path".to_owned(),
            ));
        }
        let header = git
            .repo
            .find_header(entry.id())
            .map_err(|error| fail(format!("cannot read blob header from HEAD: {error}")))?;
        if header.size() > max_bytes {
            return Ok(None);
        }
        let bytes = entry
            .object()
            .map_err(|error| fail(format!("cannot read blob from HEAD: {error}")))?
            .detach()
            .data;
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| fail(format!("HEAD blob is not UTF-8 text: {error}")))
    }

    /// The bytes of root-relative `relative` as committed in `HEAD`.
    ///
    /// Returns `None` outside Git, and an empty blob when `HEAD` has no
    /// such path.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when `HEAD` or the blob cannot be read.
    pub fn head_bytes(&self, relative: &Path) -> Result<Option<Vec<u8>>, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Ok(None);
        };
        let fail = |message: String| WorkspaceError {
            path: self.root.join(relative),
            message,
        };
        let unborn = git
            .repo
            .head()
            .map_err(|error| fail(format!("cannot read HEAD: {error}")))?
            .is_unborn();
        if unborn {
            tracing::debug!("HEAD is unborn; diff source is empty");
            return Ok(Some(Vec::new()));
        }
        let tree = git
            .repo
            .head_tree()
            .map_err(|error| fail(format!("cannot read HEAD tree: {error}")))?;
        self.blob_in(&tree, relative, "HEAD").map(Some)
    }

    /// The bytes of `relative` in `tree`, empty when the tree has no such
    /// path; `whence` names the tree in errors.
    fn blob_in(
        &self,
        tree: &gix::Tree<'_>,
        relative: &Path,
        whence: &str,
    ) -> Result<Vec<u8>, WorkspaceError> {
        let fail = |message: String| WorkspaceError {
            path: self.root.join(relative),
            message,
        };
        let Some(entry) = tree.lookup_entry_by_path(relative).map_err(|error| {
            fail(format!(
                "cannot look up {} in {whence}: {error}",
                relative.display()
            ))
        })?
        else {
            tracing::debug!(path = %relative.display(), whence, "not in the tree; text is empty");
            return Ok(Vec::new());
        };
        if !entry.mode().is_blob_or_symlink() {
            return Err(fail(format!(
                "{whence} has a directory or submodule at this path"
            )));
        }
        let object = entry
            .object()
            .map_err(|error| fail(format!("cannot read blob from {whence}: {error}")))?;
        Ok(object.detach().data)
    }

    /// The text of root-relative `relative` as staged in the index, the
    /// middle text that tells a staged hunk from an unstaged one
    /// (ADR 0017).
    ///
    /// Returns `None` outside git, and `Some("")` when the index has no
    /// such path.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the index or the blob cannot be
    /// read, or the blob is not UTF-8 text.
    pub fn index_text(&self, relative: &Path) -> Result<Option<String>, WorkspaceError> {
        let Some(bytes) = self.index_bytes(relative)? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| WorkspaceError {
                path: self.root.join(relative),
                message: format!("index blob is not UTF-8 text: {error}"),
            })
    }

    /// The bytes of root-relative `relative` as staged in the index.
    ///
    /// Returns `None` outside Git, and an empty blob when the index has no
    /// such path.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the index or blob cannot be read.
    pub fn index_bytes(&self, relative: &Path) -> Result<Option<Vec<u8>>, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Ok(None);
        };
        let fail = |message: String| WorkspaceError {
            path: self.root.join(relative),
            message,
        };
        let index = git
            .repo
            .index_or_empty()
            .map_err(|error| fail(format!("cannot read the index: {error}")))?;
        let path = gix::path::to_unix_separators_on_windows(gix::path::into_bstr(relative));
        let Some(entry) = index.entry_by_path(path.as_ref()) else {
            return Ok(Some(Vec::new()));
        };
        if !matches!(
            entry.mode,
            gix::index::entry::Mode::FILE
                | gix::index::entry::Mode::FILE_EXECUTABLE
                | gix::index::entry::Mode::SYMLINK
        ) {
            return Ok(Some(Vec::new()));
        }
        let object = git
            .repo
            .find_object(entry.id)
            .map_err(|error| fail(format!("cannot read blob from the index: {error}")))?;
        Ok(Some(object.detach().data))
    }

    /// Every uncommitted path (ADR 0017): the index against `HEAD` for
    /// staged changes, the working tree against the index for unstaged
    /// ones, and non-ignored files the index lacks as untracked. Empty
    /// outside git.
    ///
    /// A tracked file whose size and mtime match the index is taken as
    /// clean without reading it, as git does, unless its mtime is not
    /// older than the index file's own (racily clean, in git's words: a
    /// rewrite to the same size within the second of a `git add` would
    /// hide behind the match); anything else is hashed.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when `HEAD` or the index cannot be read.
    pub fn status(&mut self) -> Result<Status, WorkspaceError> {
        self.check_scan()?;
        self.begin_comparison();
        let started = std::time::Instant::now();
        let Some(git) = self.ignore.as_ref() else {
            return Ok(Status::default());
        };
        let fail = |message: String| WorkspaceError {
            path: self.root.clone(),
            message,
        };
        let index = git
            .repo
            .index_or_empty()
            .map_err(|error| fail(format!("cannot read the index: {error}")))?;
        self.check_path_count(index.entries().len())?;
        let mut head: BTreeMap<BString, ObjectId> = BTreeMap::new();
        if let Some(tree) = head_tree_of(&git.repo).map_err(&fail)? {
            collect_blobs(
                &tree,
                BString::default(),
                &mut head,
                &self.limits,
                &self.cancellation,
            )
            .map_err(|error| fail(format!("cannot walk HEAD tree: {error}")))?;
        }
        let hash = git.repo.object_hash();
        let mut dirty: BTreeMap<BString, (Option<State>, Option<State>)> = BTreeMap::new();
        let mut in_index: BTreeSet<BString> = BTreeSet::new();
        for entry in index.entries() {
            self.check_scan()?;
            if !is_status_entry(entry) {
                continue;
            }
            let path = entry.path(&index).to_owned();
            in_index.insert(path.clone());
            let staged = match head.get(&path) {
                Some(id) if *id == entry.id => None,
                Some(_) => Some(State::Modified),
                None => Some(State::Added),
            };
            let relative = gix::path::from_bstr(path.as_bstr());
            let worktree = worktree_state(self, &relative, entry, hash, &index)?;
            if Changes::from_sides(staged, worktree).is_some() {
                dirty.insert(path, (staged, worktree));
            }
        }
        for path in head.keys() {
            if !in_index.contains(path) {
                dirty.insert(path.clone(), (Some(State::Deleted), None));
            }
        }
        let discovered = self.discover_files(Filter::Visible);
        if let Some(error) = discovered.incomplete {
            return Err(error);
        }
        for file in discovered.paths {
            self.check_path_count(dirty.len().saturating_add(1))?;
            let path = BString::from(file);
            if !in_index.contains(&path) {
                dirty
                    .entry(path)
                    .and_modify(|(_, unstaged)| *unstaged = Some(State::Untracked))
                    .or_insert((None, Some(State::Untracked)));
            }
        }
        let mut entries = Vec::with_capacity(dirty.len());
        for (path, (staged, unstaged)) in dirty {
            let relative = gix::path::from_bstr(path.as_bstr()).into_owned();
            if let Some(changes) = Changes::from_sides(staged, unstaged) {
                entries.push(self.dirty_entry(relative, changes)?);
            }
        }
        let status = Status::from_entries(entries);
        tracing::debug!(dirty = status.len(), elapsed = ?started.elapsed(), "git status");
        Ok(status)
    }

    /// The dirty set after the root-relative `changed` paths moved, from
    /// `previous`, the set before they did. Only those paths are
    /// examined again, and under a directory among them its tracked
    /// files, its entries in `previous`, and the files it holds on disk;
    /// the rest of `previous` is kept. A burst of writes costs the paths
    /// it touched, not a walk of the tree (ADR 0017).
    ///
    /// The index and `HEAD` are read as they are now, but `previous`
    /// must have been built against them: a change under `.git`, or to
    /// a file [`is_rules_file`] names, takes the full walk of
    /// [`Workspace::status`] instead (reload the rules with
    /// [`Workspace::reload_rules`] first), as does one naming the root.
    /// A directory with many tracked files has its `HEAD` subtree read
    /// once rather than looked up per file. Empty outside git.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when `HEAD` or the index cannot be read.
    pub fn status_after(
        &mut self,
        previous: &Status,
        changed: &[PathBuf],
    ) -> Result<Status, WorkspaceError> {
        self.check_scan()?;
        self.begin_comparison();
        let started = std::time::Instant::now();
        let Some(git) = self.ignore.as_ref() else {
            return Ok(Status::default());
        };
        // The root named is the whole tree; a `.git` or rules change can
        // move any path: the walk is the cheaper answer for both.
        if changed.iter().any(|path| {
            path.as_os_str().is_empty() || path.starts_with(".git") || is_rules_file(path)
        }) {
            return self.status();
        }
        let repo = git.repo.clone();
        let root = self.root.clone();
        let fail = move |message: String| WorkspaceError {
            path: root.clone(),
            message,
        };
        let index = repo
            .index_or_empty()
            .map_err(|error| fail(format!("cannot read the index: {error}")))?;
        self.check_path_count(index.entries().len())?;
        self.check_path_count(previous.len())?;
        let mut head = Head {
            tree: head_tree_of(&repo).map_err(&fail)?,
            hash: repo.object_hash(),
            collected: BTreeMap::new(),
            covered: Vec::new(),
            limits: self.limits.clone(),
            cancellation: self.cancellation.clone(),
        };
        let examine = self.paths_to_examine(&index, previous, changed, &mut head)?;
        let mut entries: Vec<status::Entry> = previous
            .entries()
            .iter()
            .filter(|entry| !examine.contains(entry.path()))
            .cloned()
            .collect();
        for relative in &examine {
            self.check_path_count(entries.len().saturating_add(1))?;
            if let Some(changes) = self.dirty_state(&index, &head, previous, relative)? {
                entries.push(self.dirty_entry(relative.clone(), changes)?);
            }
        }
        let status = Status::from_entries(entries);
        tracing::debug!(
            changed = changed.len(),
            examined = examine.len(),
            dirty = status.len(),
            elapsed = ?started.elapsed(),
            "git status after changes"
        );
        Ok(status)
    }

    /// The paths [`Workspace::status_after`] examines for `changed`: each
    /// path, and under one that is a directory its tracked files, its
    /// entries in `previous`, and the files it holds on disk. A directory
    /// with many tracked files has its `HEAD` subtree read once into
    /// `head`, so its files cost a map lookup each rather than a tree
    /// walk each.
    fn paths_to_examine(
        &mut self,
        index: &gix::index::State,
        previous: &Status,
        changed: &[PathBuf],
        head: &mut Head<'_>,
    ) -> Result<BTreeSet<PathBuf>, WorkspaceError> {
        let mut examine: BTreeSet<PathBuf> = BTreeSet::new();
        for path in changed {
            self.check_scan()?;
            examine.insert(path.clone());
            let mut prefix = unix_path(path).into_owned();
            if !prefix.is_empty() {
                prefix.push(b'/');
            }
            let tracked = index.prefixed_entries(prefix.as_bstr()).unwrap_or(&[]);
            if tracked.len() >= COLLECT_SUBTREE_FROM {
                head.collect(path).map_err(|message| WorkspaceError {
                    path: self.root.join(path),
                    message,
                })?;
            }
            for entry in tracked {
                examine.insert(gix::path::from_bstr(entry.path(index)).into_owned());
                self.check_path_count(examine.len())?;
            }
            for entry in previous.entries() {
                if entry.path().starts_with(path) {
                    examine.insert(entry.path().to_path_buf());
                    self.check_path_count(examine.len())?;
                }
            }
            // A directory that appeared whole, moved in, or was written
            // without an event per file: its files by the same walk that
            // finds untracked ones. A symlink to one is a file to git.
            let is_dir = self
                .root
                .join(path)
                .symlink_metadata()
                .is_ok_and(|meta| meta.is_dir());
            if is_dir && !self.is_ignored(path, EntryKind::Dir) {
                let discovered = self.discover_files_under(path, Filter::Visible);
                if let Some(error) = discovered.incomplete {
                    return Err(error);
                }
                for found in discovered.paths {
                    examine.insert(PathBuf::from(found));
                    self.check_path_count(examine.len())?;
                }
            }
            self.check_path_count(examine.len())?;
        }
        Ok(examine)
    }

    /// How root-relative `relative` is dirty now, as the full walk of
    /// [`Workspace::status`] would find it: `None` when it is clean.
    /// `previous` stands in for the `HEAD`-only paths the walk collects,
    /// which only an index change can alter.
    fn dirty_state(
        &mut self,
        index: &gix::index::State,
        head: &Head<'_>,
        previous: &Status,
        relative: &Path,
    ) -> Result<Option<Changes>, WorkspaceError> {
        let absolute = self.root.join(relative);
        let tracked = index
            .entry_by_path(unix_path(relative).as_ref())
            .filter(|entry| is_status_entry(entry));
        if let Some(entry) = tracked {
            let staged = match head.id_of(relative).map_err(|message| WorkspaceError {
                path: absolute.clone(),
                message,
            })? {
                Some(id) if id == entry.id => None,
                Some(_) => Some(State::Modified),
                None => Some(State::Added),
            };
            return Ok(Changes::from_sides(
                staged,
                worktree_state(self, relative, entry, head.hash, index)?,
            ));
        }
        let on_disk = absolute
            .symlink_metadata()
            .is_ok_and(|meta| meta.is_file() || meta.is_symlink());
        if on_disk && !self.is_ignored(relative, EntryKind::File) {
            let staged = head
                .id_of(relative)
                .map_err(|message| WorkspaceError {
                    path: absolute,
                    message,
                })?
                .map(|_| State::Deleted);
            return Ok(Changes::from_sides(staged, Some(State::Untracked)));
        }
        // Gone from the index and now from disk too: the staged deletion
        // the file had covered, when there was one.
        if previous.contains(relative)
            && head
                .id_of(relative)
                .map_err(|message| WorkspaceError {
                    path: absolute,
                    message,
                })?
                .is_some()
        {
            return Ok(Some(Changes::Staged(State::Deleted)));
        }
        Ok(None)
    }

    /// The entry for a dirty `relative`, with its aggregate line counts.
    fn dirty_entry(
        &mut self,
        relative: PathBuf,
        changes: Changes,
    ) -> Result<status::Entry, WorkspaceError> {
        Ok(match self.count_lines(&relative)? {
            Lines::Text { added, removed } => status::Entry::new(relative, changes, added, removed),
            Lines::Binary => status::Entry::new(relative, changes, 0, 0).binary(),
        })
    }

    /// Line counts of the working tree against `HEAD`, or that the file
    /// is binary by git's rule (ADR 0026): the `diff` attribute, else a
    /// `NUL` in the first bytes of whichever side exists.
    fn count_lines(&mut self, relative: &Path) -> Result<Lines, WorkspaceError> {
        let attr = self.diff_attr(relative);
        let old = if let Some(head) = self.head_commit() {
            self.endpoint_bytes(
                &ComparisonEndpoint::Commit(
                    CommitId::parse(head).map_err(|error| self.scan_error(error.to_string()))?,
                ),
                relative,
            )?
            .unwrap_or_default()
        } else {
            Vec::new()
        };
        let new = self.worktree_bytes(relative)?.unwrap_or_default();
        let binary = attr
            .decided()
            .unwrap_or_else(|| content::is_binary(&old) || content::is_binary(&new));
        if binary {
            return Ok(Lines::Binary);
        }
        Ok(match (String::from_utf8(old), String::from_utf8(new)) {
            (Ok(old), Ok(new)) => {
                let (added, removed) = Diff::new(&old, &new).counts();
                Lines::Text { added, removed }
            }
            _ => Lines::Text {
                added: 0,
                removed: 0,
            },
        })
    }

    /// The diffable bytes of `relative` on disk: a symlink's target path,
    /// matching the blob git stores for it, or the file's content.
    fn worktree_bytes(&self, relative: &Path) -> Result<Option<Vec<u8>>, WorkspaceError> {
        let absolute = self.root.join(relative);
        match absolute.symlink_metadata() {
            Ok(meta) if meta.is_symlink() => {
                let target = fs::read_link(&absolute).map_err(|error| WorkspaceError {
                    path: absolute,
                    message: error.to_string(),
                })?;
                let bytes = target.to_string_lossy().into_owned().into_bytes();
                self.charge_content(bytes.len())?;
                Ok(Some(bytes))
            }
            _ => self.endpoint_bytes(&ComparisonEndpoint::WorkingTree, relative),
        }
    }

    /// `path` relative to the root, or as given when it lies outside.
    #[must_use]
    pub fn relative(&self, path: &Path) -> PathBuf {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        path.strip_prefix(&self.root)
            .map_or(path.clone(), Path::to_path_buf)
    }

    /// Whether git ignores the root-relative `relative`; never true outside git.
    /// The `diff` attribute `.gitattributes` gives root-relative
    /// `relative` (ADR 0026); [`Attr::Unspecified`] outside git.
    pub fn diff_attr(&mut self, relative: &Path) -> Attr {
        match self.ignore.as_mut() {
            Some(ignore) => ignore.diff_attr(relative),
            None => Attr::Unspecified,
        }
    }

    /// The size in bytes of root-relative `relative` as committed in
    /// `HEAD`, read from the object header without loading the blob
    /// (ADR 0026).
    ///
    /// Returns `None` outside git, on an unborn branch, and when `HEAD`
    /// has no blob at this path.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when `HEAD` or its tree cannot be read.
    pub fn head_size(&self, relative: &Path) -> Result<Option<u64>, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Ok(None);
        };
        let fail = |message: String| WorkspaceError {
            path: self.root.join(relative),
            message,
        };
        if git
            .repo
            .head()
            .map_err(|error| fail(format!("cannot read HEAD: {error}")))?
            .is_unborn()
        {
            return Ok(None);
        }
        let tree = git
            .repo
            .head_tree()
            .map_err(|error| fail(format!("cannot read HEAD tree: {error}")))?;
        let Some(entry) = tree.lookup_entry_by_path(relative).map_err(|error| {
            fail(format!(
                "cannot look up {} in HEAD: {error}",
                relative.display()
            ))
        })?
        else {
            return Ok(None);
        };
        if !entry.mode().is_blob_or_symlink() {
            return Ok(None);
        }
        let header = git
            .repo
            .find_header(entry.id())
            .map_err(|error| fail(format!("cannot read blob header from HEAD: {error}")))?;
        Ok(Some(header.size()))
    }

    /// Whether `relative` is excluded by the gitignore rules, as a file or
    /// as a directory.
    pub fn is_ignored(&mut self, relative: &Path, kind: EntryKind) -> bool {
        let Some(ignore) = self.ignore.as_mut() else {
            return false;
        };
        ignore.is_ignored(relative, kind)
    }

    /// The entries of the root-relative directory `relative`, sorted with
    /// directories first, hiding `.git` and ignored entries.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the directory cannot be read.
    pub fn list_dir(&mut self, relative: impl AsRef<Path>) -> Result<Vec<Entry>, WorkspaceError> {
        self.list_dir_with(relative.as_ref(), Filter::Visible)
    }

    /// Like [`Workspace::list_dir`] with an explicit [`Filter`].
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the directory cannot be read.
    pub(crate) fn list_dir_with(
        &mut self,
        relative: &Path,
        filter: Filter,
    ) -> Result<Vec<Entry>, WorkspaceError> {
        self.list_dir_with_limit(relative, filter, self.limits.retained_paths)
    }

    pub(crate) fn list_dir_with_limit(
        &mut self,
        relative: &Path,
        filter: Filter,
        retained_limit: usize,
    ) -> Result<Vec<Entry>, WorkspaceError> {
        let dir = self.root.join(relative);
        let read = fs::read_dir(&dir).map_err(|source| WorkspaceError {
            path: dir.clone(),
            message: format!("cannot read directory: {source}"),
        })?;
        let mut entries = Vec::new();
        for (examined, item) in read.enumerate() {
            self.check_scan()?;
            if examined >= self.limits.discovery_entries
                || self
                    .listing_examined
                    .is_some_and(|count| count >= self.limits.discovery_entries)
                || entries.len() >= retained_limit
            {
                self.listing_incomplete = Some(self.scan_error(
                    "directory listing limited by discovery budget; coverage is incomplete",
                ));
                break;
            }
            if let Some(examined) = &mut self.listing_examined {
                *examined += 1;
            }
            let item = match item {
                Ok(item) => item,
                Err(error) => {
                    tracing::warn!(%error, dir = %dir.display(), "skipping unreadable entry");
                    self.listing_incomplete = Some(self.scan_error(format!(
                        "cannot enumerate directory: {error}; coverage is incomplete"
                    )));
                    continue;
                }
            };
            let name = item.file_name();
            let Some(name) = name.to_str() else {
                tracing::debug!(dir = %dir.display(), "skipping non-UTF-8 file name");
                self.listing_incomplete =
                    Some(self.scan_error("non-UTF-8 file name; coverage is incomplete"));
                continue;
            };
            let name = name.to_owned();
            if name == ".git" {
                continue;
            }
            // Browsing may follow a symlink into its target directory,
            // but git never does: for ignore rules and the dirty set the
            // link is a file-like entry, wherever it points.
            let file_type = match item.file_type() {
                Ok(file_type) => Some(file_type),
                Err(error) => {
                    tracing::warn!(
                        %error,
                        path = %item.path().display(),
                        "cannot inspect directory entry"
                    );
                    self.listing_incomplete = Some(self.scan_error(format!(
                        "cannot inspect entry: {error}; coverage is incomplete"
                    )));
                    None
                }
            };
            let is_link = file_type.is_some_and(|kind| kind.is_symlink());
            let is_dir = if is_link {
                item.path().is_dir()
            } else {
                file_type.is_some_and(|kind| kind.is_dir())
            };
            let kind = if is_dir && !is_link {
                EntryKind::Dir
            } else {
                EntryKind::File
            };
            if filter == Filter::Visible && self.is_ignored(&relative.join(&name), kind) {
                continue;
            }
            entries.push(Entry {
                name,
                is_dir,
                is_link,
            });
        }
        entries.sort_by_cached_key(|entry| {
            (!entry.is_dir, entry.name.to_lowercase(), entry.name.clone())
        });
        Ok(entries)
    }

    /// Every file under the root as a root-relative path: a directory's
    /// files in listing order, then its subdirectories in listing order,
    /// honouring the same filters as [`Workspace::list_dir`]. A symlink
    /// is a file here and is never descended, as `git status` treats it.
    ///
    /// Directories that cannot be read are logged and skipped.
    pub fn walk_files(&mut self, filter: Filter) -> Vec<String> {
        self.walk_files_under(Path::new(""), filter)
    }

    /// Every file under the root, refusing an incomplete traversal.
    ///
    /// Comparison and review-point capture use this path so an unreadable
    /// directory cannot be mistaken for verified deletion.
    fn walk_files_checked(&mut self, filter: Filter) -> Result<Vec<PathBuf>, WorkspaceError> {
        self.walk_files_checked_under(Path::new(""), filter)
    }

    fn walk_files_checked_under(
        &mut self,
        root: &Path,
        filter: Filter,
    ) -> Result<Vec<PathBuf>, WorkspaceError> {
        let found = self.discover_files_under(root, filter);
        if let Some(error) = found.incomplete {
            return Err(error);
        }
        Ok(found.paths.into_iter().map(PathBuf::from).collect())
    }

    /// Discover files with finite examined-entry and retained-path budgets.
    ///
    /// Partial results remain useful, but [`Discovery::incomplete`] must be
    /// shown whenever it is present. Directory symlinks are never descended.
    pub fn discover_files(&mut self, filter: Filter) -> Discovery {
        self.discover_files_under(Path::new(""), filter)
    }

    fn discover_files_under(&mut self, root: &Path, filter: Filter) -> Discovery {
        let mut found = Discovery {
            paths: Vec::new(),
            incomplete: None,
        };
        let mut pending = vec![root.to_path_buf()];
        let mut examined = 0;
        let result = (|| {
            while let Some(dir) = pending.pop() {
                self.check_scan()?;
                let read = fs::read_dir(self.root.join(&dir)).map_err(|error| WorkspaceError {
                    path: self.root.join(&dir),
                    message: format!("cannot enumerate directory: {error}; coverage is incomplete"),
                })?;
                for item in read {
                    self.check_scan()?;
                    if examined >= self.limits.discovery_entries {
                        return Err(self.scan_error(
                            "discovery limited by examined-entry budget; coverage is incomplete",
                        ));
                    }
                    examined += 1;
                    let item = item.map_err(|error| {
                        self.scan_error(format!(
                            "cannot enumerate entry: {error}; coverage is incomplete"
                        ))
                    })?;
                    if item.file_name() == ".git" {
                        continue;
                    }
                    let path = dir.join(item.file_name());
                    let kind = item.file_type().map_err(|error| {
                        self.scan_error(format!(
                            "cannot inspect entry: {error}; coverage is incomplete"
                        ))
                    })?;
                    let is_dir = kind.is_dir() && !kind.is_symlink();
                    if filter == Filter::Visible
                        && self.is_ignored(
                            &path,
                            if is_dir {
                                EntryKind::Dir
                            } else {
                                EntryKind::File
                            },
                        )
                    {
                        continue;
                    }
                    if pending.len().saturating_add(found.paths.len()) >= self.limits.retained_paths
                    {
                        return Err(self.scan_error(
                            "discovery limited by retained-path budget; coverage is incomplete",
                        ));
                    }
                    if is_dir {
                        pending.push(path);
                    } else {
                        let text = path.to_str().ok_or_else(|| {
                            self.scan_error("non-UTF-8 path; coverage is incomplete")
                        })?;
                        found.paths.push(text.to_owned());
                    }
                }
            }
            Ok(())
        })();
        found.incomplete = result.err();
        found.paths.sort_by(|a, b| walk_order(a, b));
        found
    }

    /// The files under root-relative `dir` as [`Workspace::walk_files`]
    /// lists them, `dir` itself listed first: what a directory that
    /// arrived whole adds to an index kept in [`walk_order`].
    pub fn walk_files_under(&mut self, dir: &Path, filter: Filter) -> Vec<String> {
        let found = self.discover_files_under(dir, filter);
        if let Some(error) = found.incomplete {
            tracing::warn!(%error, "incomplete file index");
        }
        found.paths
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ancestry {
    FirstParents(usize),
    Parent(usize),
}

fn parent_expression(spec: &str) -> Option<(&str, Ancestry)> {
    if let Some((base, suffix)) = spec.rsplit_once('~')
        && !base.is_empty()
    {
        let count = if suffix.is_empty() {
            1
        } else {
            suffix.parse().ok()?
        };
        return Some((base, Ancestry::FirstParents(count)));
    }
    if let Some((base, suffix)) = spec.rsplit_once('^')
        && !base.is_empty()
    {
        let count = if suffix.is_empty() {
            1
        } else {
            suffix.parse().ok()?
        };
        return Some((base, Ancestry::Parent(count)));
    }
    None
}

fn check_commit_header(repo: &gix::Repository, id: ObjectId) -> Result<(), String> {
    let header = repo
        .find_header(id)
        .map_err(|error| format!("cannot inspect commit object: {error}"))?;
    if header.kind() != gix::objs::Kind::Commit {
        return Err(format!("{id} is not a commit object"));
    }
    check_exact_metadata_size(header.size(), "commit object")
}

fn check_exact_metadata_size(size: u64, object: &str) -> Result<(), String> {
    if size > MAX_EXACT_METADATA_BYTES {
        return Err(format!(
            "{object} exceeds the {MAX_EXACT_METADATA_BYTES}-byte metadata limit"
        ));
    }
    Ok(())
}

fn commit_record(repo: &gix::Repository, id: ObjectId) -> Result<Commit, String> {
    let commit = repo
        .find_commit(id)
        .map_err(|error| format!("cannot read commit object: {error}"))?;
    let time = commit
        .time()
        .map_err(|error| format!("cannot read commit time: {error}"))?
        .seconds;
    let time = u64::try_from(time).unwrap_or(0);
    let subject = commit
        .message_raw_sloppy()
        .lines()
        .next()
        .map(|line| line.to_str_lossy().into_owned())
        .unwrap_or_default();
    let parents = commit
        .parent_ids()
        .map(|parent| parent.detach().to_hex().to_string())
        .collect();
    Ok(Commit {
        hex: id.to_hex().to_string(),
        time,
        subject,
        parents,
    })
}

fn named_revision_choice(
    reference: &mut gix::Reference<'_>,
    kind: RevisionChoiceKind,
    remote: bool,
) -> Option<RevisionChoice> {
    let name = reference.name().shorten().to_str_lossy().into_owned();
    let commit = match reference.peel_to_commit() {
        Ok(commit) => commit,
        Err(error) => {
            tracing::debug!(
                reference = %name,
                %error,
                "skipping local revision that does not resolve to a commit"
            );
            return None;
        }
    };
    Some(RevisionChoice {
        kind,
        name,
        commit: CommitId(commit.id.to_hex().to_string()),
        remote,
    })
}

fn collect_endpoint_tree(
    workspace: &Workspace,
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    out: &mut BTreeMap<PathBuf, EndpointFile>,
) -> Result<(), String> {
    let mut pending = vec![(tree.id, PathBuf::new())];
    while let Some((id, prefix)) = pending.pop() {
        workspace.check_scan().map_err(|error| error.to_string())?;
        let tree = repo.find_tree(id).map_err(|error| error.to_string())?;
        for entry in tree.iter() {
            workspace
                .check_path_count(out.len().saturating_add(1))
                .map_err(|error| error.to_string())?;
            let entry = entry.map_err(|error| error.to_string())?;
            let path = prefix.join(gix::path::from_bstr(entry.filename()));
            let mode = tree_mode(entry.mode());
            if mode.is_directory() {
                out.insert(
                    path.clone(),
                    EndpointFile {
                        info: PathInfo::new(
                            mode,
                            None,
                            Some(entry.object_id().to_hex().to_string()),
                            false,
                        )
                        .with_supported(endpoint_size_supported(mode, None)),
                    },
                );
                pending.push((entry.object_id(), path));
                continue;
            }
            let size = mode
                .is_supported()
                .then(|| {
                    repo.find_header(entry.object_id())
                        .map(|header| header.size())
                })
                .transpose()
                .map_err(|error| error.to_string())?;
            out.insert(
                path,
                EndpointFile {
                    info: PathInfo::new(
                        mode,
                        size,
                        Some(entry.object_id().to_hex().to_string()),
                        false,
                    )
                    .with_supported(endpoint_size_supported(mode, size)),
                },
            );
        }
    }
    Ok(())
}

fn tree_mode(mode: gix::objs::tree::EntryMode) -> FileMode {
    match mode.kind() {
        gix::objs::tree::EntryKind::Blob => FileMode::Regular,
        gix::objs::tree::EntryKind::BlobExecutable => FileMode::Executable,
        gix::objs::tree::EntryKind::Link => FileMode::Symlink,
        gix::objs::tree::EntryKind::Tree => FileMode::Directory,
        gix::objs::tree::EntryKind::Commit => FileMode::Submodule,
    }
}

fn index_mode(mode: gix::index::entry::Mode) -> FileMode {
    match mode {
        gix::index::entry::Mode::FILE => FileMode::Regular,
        gix::index::entry::Mode::FILE_EXECUTABLE => FileMode::Executable,
        gix::index::entry::Mode::SYMLINK => FileMode::Symlink,
        gix::index::entry::Mode::DIR => FileMode::Directory,
        gix::index::entry::Mode::COMMIT => FileMode::Submodule,
        other => FileMode::Other(other.bits()),
    }
}

fn git_mode(mode: FileMode) -> u32 {
    match mode {
        FileMode::Directory => 0o040_000,
        FileMode::Regular => 0o100_644,
        FileMode::Executable => 0o100_755,
        FileMode::Symlink => 0o120_000,
        FileMode::Submodule => 0o160_000,
        FileMode::Other(raw) => raw,
    }
}

fn manifest_identity(entries: &[IndexManifestEntry]) -> String {
    use fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(b"fathomable-index-manifest-v1\0");
    hasher.update(
        u64::try_from(entries.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    for entry in entries {
        hasher.update(
            u64::try_from(entry.path.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        hasher.update(entry.path.as_ref());
        hasher.update(git_mode(entry.mode).to_be_bytes());
        hasher.update(
            u64::try_from(entry.object.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        hasher.update(entry.object.as_bytes());
    }
    let mut out = String::with_capacity(64);
    for byte in hasher.finalize() {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn valid_git_path(path: &[u8]) -> bool {
    !path.is_empty()
        && !path.starts_with(b"/")
        && !path.ends_with(b"/")
        && !path.contains(&0)
        && path
            .split(|byte| *byte == b'/')
            .all(|part| !part.is_empty() && part != b"." && part != b"..")
}

fn working_tree_mode(metadata: &fs::Metadata) -> FileMode {
    if metadata.is_symlink() {
        return FileMode::Symlink;
    }
    if metadata.is_file() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 != 0 {
                return FileMode::Executable;
            }
        }
        return FileMode::Regular;
    }
    FileMode::Other(0)
}

fn endpoint_size_supported(mode: FileMode, size: Option<u64>) -> bool {
    mode.is_supported() && size.is_none_or(|size| size <= content::Policy::default().max_bytes)
}

impl Ignore {
    fn new(repo: &gix::Repository) -> Result<Self, String> {
        let index = repo
            .index_or_empty()
            .map_err(|error| format!("cannot read git index: {error}"))?;
        let stack = repo
            .attributes(
                &index,
                AttrSource::WorktreeThenIdMapping,
                Source::WorktreeThenIdMappingIfNotSkipped,
                None,
            )
            .map_err(|error| format!("cannot read git ignore rules and attributes: {error}"))?
            .detach();
        let diff_attr = stack.selected_attribute_matches(["diff"]);
        Ok(Self {
            repo: repo.clone(),
            stack,
            diff_attr,
        })
    }

    /// The `diff` attribute of `relative` (ADR 0026).
    fn diff_attr(&mut self, relative: &Path) -> Attr {
        let platform = match self.stack.at_path(
            relative,
            Some(gix::index::entry::Mode::FILE),
            &gix::objs::find::Never,
        ) {
            Ok(platform) => platform,
            Err(error) => {
                tracing::warn!(%error, path = %relative.display(), "attribute lookup failed");
                return Attr::Unspecified;
            }
        };
        self.diff_attr.reset();
        platform.matching_attributes(&mut self.diff_attr);
        let state = self
            .diff_attr
            .iter_selected()
            .find(|m| m.assignment.name.as_str() == "diff")
            .map(|m| m.assignment.state);
        match state {
            Some(gix::attrs::StateRef::Unset) => Attr::Binary,
            Some(gix::attrs::StateRef::Set | gix::attrs::StateRef::Value(_)) => Attr::Text,
            Some(gix::attrs::StateRef::Unspecified) | None => Attr::Unspecified,
        }
    }

    fn is_ignored(&mut self, relative: &Path, kind: EntryKind) -> bool {
        let mode = match kind {
            EntryKind::Dir => gix::index::entry::Mode::DIR,
            EntryKind::File => gix::index::entry::Mode::FILE,
        };
        // Ignore files are read from the work tree; the object database is
        // never consulted, so the finder can be the one that finds nothing.
        match self
            .stack
            .at_path(relative, Some(mode), &gix::objs::find::Never)
        {
            Ok(platform) => platform.is_excluded(),
            Err(error) => {
                tracing::warn!(%error, path = %relative.display(), "ignore lookup failed");
                false
            }
        }
    }
}

/// What `count_lines` found for a dirty path.
enum Lines {
    Text { added: usize, removed: usize },
    Binary,
}

/// Why the workspace could not be opened or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceError {
    path: PathBuf,
    message: String,
}

impl WorkspaceError {
    /// The path the failure concerns.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The actionable failure detail without the path prefix.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

impl std::error::Error for WorkspaceError {}

impl From<WorkspaceError> for io::Error {
    fn from(error: WorkspaceError) -> Self {
        Self::other(error)
    }
}

/// Whether an edit to root-relative `relative` changes what the ignore
/// rules or attributes say about other paths: a `.gitignore` or
/// `.gitattributes` anywhere under the root, or `.git/info/exclude`.
#[must_use]
pub fn is_rules_file(relative: &Path) -> bool {
    relative == Path::new(".git/info/exclude")
        || relative
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| matches!(name, ".gitignore" | ".gitattributes"))
}

/// Directory listings put directories first, then names case-insensitively.
pub(crate) fn entry_order(a_dir: bool, a: &str, b_dir: bool, b: &str) -> Ordering {
    b_dir
        .cmp(&a_dir)
        .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
        .then_with(|| a.cmp(b))
}

/// The order [`Workspace::walk_files`] lists root-relative paths in: at
/// each level a directory's files come before its subdirectories, and
/// names sort by their lowercase form, then as written. An index kept
/// in this order takes a new path at its partition point.
///
/// ```
/// use std::cmp::Ordering;
/// use fathomable_core::workspace::walk_order;
///
/// let mut paths = ["src/lib.rs", "Cargo.toml", "src/app/mod.rs", "README.md", "src/b.rs"];
/// paths.sort_by(|a, b| walk_order(a, b));
/// assert_eq!(paths, ["Cargo.toml", "README.md", "src/b.rs", "src/lib.rs", "src/app/mod.rs"]);
/// assert_eq!(walk_order("a/b", "a/b"), Ordering::Equal);
/// ```
#[must_use]
pub fn walk_order(a: &str, b: &str) -> Ordering {
    let mut left = a.split('/').peekable();
    let mut right = b.split('/').peekable();
    loop {
        let (Some(l), Some(r)) = (left.next(), right.next()) else {
            return Ordering::Equal;
        };
        // The last component is a file, an earlier one a directory.
        let l_file = left.peek().is_none();
        let r_file = right.peek().is_none();
        let step = match (l_file, r_file) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => l
                .to_lowercase()
                .cmp(&r.to_lowercase())
                .then_with(|| l.cmp(r)),
        };
        if step != Ordering::Equal {
            return step;
        }
    }
}

/// How every repository is opened: as git would, except that `GIT_DIR`,
/// `GIT_INDEX_FILE`, and the other `GIT_*` overrides are ignored. The
/// workspace is the path the user gave, so a viewer started from a git
/// hook (which exports those) still looks at that path's own repository.
#[must_use]
fn open_options() -> gix::open::Options {
    let mut permissions = gix::open::Permissions::default();
    permissions.env.git_prefix = gix::sec::Permission::Deny;
    gix::open::Options::default().permissions(permissions)
}

fn exact_open_options(max_object_bytes: u64) -> gix::open::Options {
    let max_object_bytes = usize::try_from(max_object_bytes).unwrap_or(usize::MAX);
    open_options().config_overrides([format!("gitoxide.objects.allocLimit={max_object_bytes}")])
}

/// How many tracked files under a changed directory make reading its
/// `HEAD` subtree once cheaper than looking each up: a lookup decodes a
/// tree per path component, and from this many on the walk wins.
const COLLECT_SUBTREE_FROM: usize = 16;

/// `HEAD` as [`Workspace::status_after`] reads it: its tree, `None`
/// while it is unborn, and the hash the repository keys objects by.
struct Head<'repo> {
    tree: Option<gix::Tree<'repo>>,
    hash: gix::hash::Kind,
    /// The blobs of every subtree read whole, by path.
    collected: BTreeMap<BString, ObjectId>,
    /// The directory prefixes `collected` covers, each ending in `/`
    /// (empty for the root).
    covered: Vec<BString>,
    limits: crate::config::LimitsConfig,
    cancellation: Cancellation,
}

impl Head<'_> {
    /// Read the subtree at root-relative `dir` once, so every lookup
    /// under it is a map hit. Nothing under `dir` in `HEAD` still counts
    /// as covered: such lookups answer `None` without a tree walk.
    fn collect(&mut self, dir: &Path) -> Result<(), String> {
        let mut prefix = unix_path(dir).into_owned();
        if !prefix.is_empty() {
            prefix.push(b'/');
        }
        if let Some(tree) = self.tree.as_ref() {
            let subtree = if dir.as_os_str().is_empty() {
                Some(tree.clone())
            } else {
                tree.lookup_entry_by_path(dir)
                    .map_err(|error| format!("cannot look up {} in HEAD: {error}", dir.display()))?
                    .filter(|entry| entry.mode().is_tree())
                    .map(|entry| entry.object().map(gix::Object::into_tree))
                    .transpose()
                    .map_err(|error| format!("cannot read {} from HEAD: {error}", dir.display()))?
            };
            if let Some(subtree) = subtree {
                collect_blobs(
                    &subtree,
                    prefix.clone(),
                    &mut self.collected,
                    &self.limits,
                    &self.cancellation,
                )
                .map_err(|error| format!("cannot walk HEAD tree: {error}"))?;
            }
        }
        self.covered.push(prefix);
        Ok(())
    }

    /// The blob or symlink `HEAD` holds at root-relative `relative`.
    fn id_of(&self, relative: &Path) -> Result<Option<ObjectId>, String> {
        let Some(tree) = self.tree.as_ref() else {
            return Ok(None);
        };
        let path = unix_path(relative);
        if self
            .covered
            .iter()
            .any(|dir| path.starts_with(dir.as_ref()))
        {
            return Ok(self.collected.get(path.as_ref()).copied());
        }
        let entry = tree
            .lookup_entry_by_path(relative)
            .map_err(|error| format!("cannot look up {} in HEAD: {error}", relative.display()))?;
        Ok(entry
            .filter(|entry| entry.mode().is_blob_or_symlink())
            .map(|entry| entry.object_id()))
    }
}

/// The tree `HEAD` points at, `None` while `HEAD` is unborn.
fn head_tree_of(repo: &gix::Repository) -> Result<Option<gix::Tree<'_>>, String> {
    let unborn = repo
        .head()
        .map_err(|error| format!("cannot read HEAD: {error}"))?
        .is_unborn();
    if unborn {
        return Ok(None);
    }
    repo.head_tree()
        .map(Some)
        .map_err(|error| format!("cannot read HEAD tree: {error}"))
}

/// Whether an index entry takes part in the dirty set: a file or symlink
/// without a merge conflict. Submodules and conflicted stages are left
/// out, as the tree pane has nothing to show for them.
fn is_status_entry(entry: &gix::index::Entry) -> bool {
    entry.stage() == gix::index::entry::Stage::Unconflicted
        && matches!(
            entry.mode,
            gix::index::entry::Mode::FILE
                | gix::index::entry::Mode::FILE_EXECUTABLE
                | gix::index::entry::Mode::SYMLINK
        )
}

/// Root-relative `relative` as the slash-separated path the index keys.
fn unix_path(relative: &Path) -> std::borrow::Cow<'_, gix::bstr::BStr> {
    gix::path::to_unix_separators_on_windows(gix::path::into_bstr(relative))
}

/// How the working tree differs from index `entry` at `absolute`: `None`
/// when they match, otherwise the [`State`] the entry is in.
///
/// A tracked file whose size and mtime match the index is taken as clean
/// without reading it, as git does, unless the entry is racily clean: its
/// mtime is not older than `index`'s own, so a rewrite to the same size
/// in the same second could hide behind the match, and the file is
/// hashed as git would. A symlink's blob is its target path, so that is
/// what gets hashed, never the file the link points at.
fn worktree_state(
    workspace: &Workspace,
    relative: &Path,
    entry: &gix::index::Entry,
    hash: gix::hash::Kind,
    index: &gix::index::State,
) -> Result<Option<State>, WorkspaceError> {
    workspace.check_scan()?;
    let absolute = workspace.root.join(relative);
    let is_link = entry.mode == gix::index::entry::Mode::SYMLINK;
    Ok(
        match gix::index::fs::Metadata::from_path_no_follow(&absolute) {
            Ok(meta) if meta.is_file() && !is_link => {
                let racy = entry.stat.is_racy(
                    index.timestamp(),
                    gix::index::entry::stat::Options::default(),
                );
                let fresh = !racy
                    && gix::index::entry::Stat::from_fs(&meta)
                        .ok()
                        .is_some_and(|stat| {
                            stat.size == entry.stat.size
                                && stat.mtime == entry.stat.mtime
                                && entry.stat.mtime.secs != 0
                        });
                if fresh {
                    None
                } else {
                    let same = workspace.worktree_bytes(relative)?.is_some_and(|data| {
                        gix::objs::compute_hash(hash, gix::objs::Kind::Blob, &data)
                            .is_ok_and(|id| id == entry.id)
                    });
                    (!same).then_some(State::Modified)
                }
            }
            Ok(meta) if meta.is_symlink() && is_link => {
                let same = workspace.worktree_bytes(relative)?.is_some_and(|target| {
                    gix::objs::compute_hash(hash, gix::objs::Kind::Blob, &target)
                        .is_ok_and(|id| id == entry.id)
                });
                (!same).then_some(State::Modified)
            }
            // A file became a symlink or the other way around.
            Ok(meta) if meta.is_file() || meta.is_symlink() => Some(State::Modified),
            _ => Some(State::Deleted),
        },
    )
}

fn collect_blobs(
    tree: &gix::Tree<'_>,
    prefix: BString,
    out: &mut BTreeMap<BString, ObjectId>,
    limits: &crate::config::LimitsConfig,
    cancellation: &Cancellation,
) -> Result<(), String> {
    let mut pending = vec![(tree.clone(), prefix)];
    let mut examined = 0usize;
    while let Some((tree, prefix)) = pending.pop() {
        for entry in tree.iter() {
            if cancellation.is_cancelled() {
                return Err("Git discovery cancelled; coverage is incomplete".to_owned());
            }
            if examined >= limits.discovery_entries {
                return Err(
                    "Git discovery limited by examined-entry budget; coverage is incomplete"
                        .to_owned(),
                );
            }
            examined += 1;
            let entry = entry.map_err(|error| error.to_string())?;
            let mut path = prefix.clone();
            path.extend_from_slice(entry.filename());
            if out.len().saturating_add(pending.len()) >= limits.comparison_path_limit() {
                return Err(
                    "Git discovery limited by retained-path budget; coverage is incomplete"
                        .to_owned(),
                );
            }
            if entry.mode().is_tree() {
                let subtree = entry
                    .object()
                    .map_err(|error| error.to_string())?
                    .into_tree();
                path.push(b'/');
                pending.push((subtree, path));
            } else if entry.mode().is_blob_or_symlink() {
                out.insert(path, entry.object_id());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use fathomable_testing::TempDir;

    use super::*;
    use fathomable_testing::git::{commit_and_stage, init, stage};

    fn head(root: &Path) -> Result<CommitId, Box<dyn std::error::Error>> {
        let repo = gix::open_opts(root, open_options())?;
        Ok(CommitId::parse(repo.head_id()?.to_hex().to_string())?)
    }

    #[test]
    fn direct_commit_comparison_includes_historical_additions_and_deletions()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("workspace-comparison-history")?;
        init(&dir.0)?;
        commit_and_stage(
            &dir.0,
            &[
                ("a.md", "one\n"),
                ("gone.md", "gone\n"),
                ("dir/old.md", "old\n"),
            ],
        )?;
        let first = head(&dir.0)?;
        commit_and_stage(
            &dir.0,
            &[
                ("a.md", "two\n"),
                ("new.md", "new\n"),
                ("dir/new.md", "new\n"),
            ],
        )?;
        let second = head(&dir.0)?;
        let mut workspace = Workspace::discover(&dir.0)?;

        let comparison = workspace.compare(
            ComparisonEndpoint::Commit(first.clone()),
            ComparisonEndpoint::Commit(second.clone()),
        )?;
        let paths: Vec<_> = comparison
            .changes()
            .iter()
            .map(|change| (change.path().to_owned(), change.kind()))
            .collect();
        assert_eq!(
            paths,
            vec![
                (
                    PathBuf::from("a.md"),
                    crate::diff::PathChangeKind::ContentChanged
                ),
                (
                    PathBuf::from("dir/new.md"),
                    crate::diff::PathChangeKind::Added
                ),
                (
                    PathBuf::from("dir/old.md"),
                    crate::diff::PathChangeKind::Deleted
                ),
                (
                    PathBuf::from("gone.md"),
                    crate::diff::PathChangeKind::Deleted
                ),
                (PathBuf::from("new.md"), crate::diff::PathChangeKind::Added),
            ]
        );
        assert_eq!(
            workspace.endpoint_text(&ComparisonEndpoint::Commit(first), Path::new("gone.md"))?,
            Some("gone\n".to_owned())
        );
        assert_eq!(
            workspace.endpoint_text(&ComparisonEndpoint::Commit(second), Path::new("gone.md"))?,
            None
        );
        Ok(())
    }

    #[test]
    fn root_and_empty_tree_comparisons_are_explicit() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("workspace-comparison-root")?;
        init(&dir.0)?;
        fs::write(dir.0.join("root.md"), "root\n")?;
        let mut workspace = Workspace::discover(&dir.0)?;
        let empty = workspace.compare(
            ComparisonEndpoint::EmptyTree,
            ComparisonEndpoint::WorkingTree,
        )?;
        assert_eq!(empty.changes().len(), 1);
        assert_eq!(
            empty.changes()[0].kind(),
            crate::diff::PathChangeKind::Added
        );

        commit_and_stage(&dir.0, &[("root.md", "root\n")])?;
        let commit = head(&dir.0)?;
        let root = workspace.compare(
            ComparisonEndpoint::EmptyTree,
            ComparisonEndpoint::Commit(commit),
        )?;
        assert_eq!(root.changes().len(), 1);
        assert_eq!(root.changes()[0].path(), Path::new("root.md"));
        Ok(())
    }

    #[test]
    fn revisions_resolve_branches_tags_ids_and_reject_non_commits()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("workspace-revisions")?;
        init(&dir.0)?;
        commit_and_stage(&dir.0, &[("a.md", "a\n")])?;
        let commit = head(&dir.0)?;
        let repo = gix::open_opts(&dir.0, open_options())?;
        repo.tag_reference(
            "v1",
            ObjectId::from_hex(commit.as_str().as_bytes())?,
            gix::refs::transaction::PreviousValue::MustNotExist,
        )?;
        let workspace = Workspace::discover(&dir.0)?;
        assert_eq!(workspace.resolve_revision("HEAD")?.id(), commit);
        assert_eq!(workspace.resolve_revision("main")?.id(), commit);
        assert_eq!(workspace.resolve_revision("v1")?.id(), commit);
        assert_eq!(workspace.resolve_revision(commit.short())?.id(), commit);
        let invalid = workspace
            .resolve_revision("not-a-revision")
            .err()
            .ok_or("invalid revision was accepted")?;
        assert!(invalid.message().contains("no local branch or tag"));
        let blob = repo.write_blob(b"not a commit")?.detach();
        let error = workspace
            .resolve_revision(blob.to_hex().to_string())
            .err()
            .ok_or("blob revision was accepted")?;
        assert!(error.to_string().contains("not a commit"));
        Ok(())
    }

    #[test]
    fn numbered_caret_selects_one_merge_parent() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("workspace-numbered-parent")?;
        init(&dir.0)?;
        commit_and_stage(&dir.0, &[("a.md", "a\n")])?;
        let first = head(&dir.0)?;
        commit_and_stage(&dir.0, &[("a.md", "b\n")])?;
        let main_parent = head(&dir.0)?;
        let repo = gix::open_opts(&dir.0, open_options())?;
        let signature = gix::actor::SignatureRef {
            name: "test".into(),
            email: "test@example.com".into(),
            time: "2 +0000",
        };
        let tree = repo.head_tree_id()?.detach();
        let side_parent = repo
            .commit_as(
                signature,
                signature,
                "refs/heads/side",
                "side",
                tree,
                Some(ObjectId::from_hex(first.as_str().as_bytes())?),
            )?
            .detach();
        repo.commit_as(
            signature,
            signature,
            "HEAD",
            "merge",
            tree,
            [
                ObjectId::from_hex(main_parent.as_str().as_bytes())?,
                side_parent,
            ],
        )?;

        let workspace = Workspace::discover(&dir.0)?;
        assert_eq!(workspace.resolve_revision("HEAD^1")?.id(), main_parent);
        assert_eq!(
            workspace.resolve_revision("HEAD^2")?.id(),
            CommitId(side_parent.to_hex().to_string())
        );
        assert_eq!(workspace.resolve_revision("HEAD~2")?.id(), first);
        assert!(
            workspace
                .resolve_revision("HEAD^3")
                .is_err_and(|error| error.message().contains("parent number 3"))
        );
        Ok(())
    }

    #[test]
    fn revision_choices_list_branches_and_peel_both_tag_kinds()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("workspace-revision-choices")?;
        init(&dir.0)?;
        commit_and_stage(&dir.0, &[("a.md", "a\n")])?;
        let commit = head(&dir.0)?;
        let repo = gix::open_opts(&dir.0, open_options())?;
        let commit_object = ObjectId::from_hex(commit.as_str().as_bytes())?;
        let signature = gix::actor::SignatureRef {
            name: "test".into(),
            email: "test@example.com".into(),
            time: "1 +0000",
        };
        repo.commit_as(
            signature,
            signature,
            "refs/heads/feature",
            "feature",
            repo.head_tree_id()?.detach(),
            Some(commit_object),
        )?;
        repo.commit_as(
            signature,
            signature,
            "refs/remotes/origin/review",
            "remote review",
            repo.head_tree_id()?.detach(),
            Some(commit_object),
        )?;
        repo.tag_reference(
            "lightweight",
            commit_object,
            gix::refs::transaction::PreviousValue::MustNotExist,
        )?;
        let annotated = repo
            .write_object(gix::objs::Tag {
                target: commit_object,
                target_kind: gix::objs::Kind::Commit,
                name: "annotated".into(),
                tagger: None,
                message: "annotated tag\n".into(),
                signature: None,
            })?
            .detach();
        repo.tag_reference(
            "annotated",
            annotated,
            gix::refs::transaction::PreviousValue::MustNotExist,
        )?;

        let workspace = Workspace::discover(&dir.0)?;
        let choices = workspace.revision_choices()?;
        let names: Vec<_> = choices
            .iter()
            .map(|choice| (choice.kind(), choice.name().to_owned()))
            .collect();
        assert_eq!(
            names,
            vec![
                (RevisionChoiceKind::Branch, "feature".to_owned()),
                (RevisionChoiceKind::Branch, "main".to_owned()),
                (RevisionChoiceKind::Branch, "origin/review".to_owned()),
                (RevisionChoiceKind::Tag, "annotated".to_owned()),
                (RevisionChoiceKind::Tag, "lightweight".to_owned()),
            ]
        );
        for name in ["annotated", "lightweight"] {
            let choice = choices
                .iter()
                .find(|choice| choice.kind() == RevisionChoiceKind::Tag && choice.name() == name)
                .ok_or("missing tag choice")?;
            assert_eq!(choice.commit(), &commit);
        }
        assert_eq!(
            choices
                .iter()
                .find(|choice| choice.name() == "feature")
                .map(RevisionChoice::kind),
            Some(RevisionChoiceKind::Branch)
        );
        assert_eq!(
            choices
                .iter()
                .find(|choice| choice.name() == "origin/review")
                .map(RevisionChoice::kind),
            Some(RevisionChoiceKind::Branch)
        );
        assert!(
            choices
                .iter()
                .find(|choice| choice.name() == "origin/review")
                .is_some_and(RevisionChoice::is_remote_branch)
        );
        assert_eq!(workspace.commits_from("origin/review", 0, 500)?.len(), 2);
        Ok(())
    }

    #[test]
    fn recent_head_commits_use_explicit_history() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("workspace-commit-picker")?;
        init(&dir.0)?;
        commit_and_stage(&dir.0, &[("a.md", "0\n")])?;
        let first = head(&dir.0)?;
        commit_and_stage(&dir.0, &[("a.md", "1\n")])?;
        commit_and_stage(&dir.0, &[("b.md", "2\n")])?;
        let last = head(&dir.0)?;
        let workspace = Workspace::discover(&dir.0)?;
        assert_eq!(workspace.recent_commits(0, 2)?.len(), 2);
        assert_eq!(workspace.commits_from("HEAD", 0, 500)?.len(), 3);
        assert_eq!(workspace.commits_from("HEAD~1", 0, 500)?.len(), 2);
        assert_eq!(
            workspace.commits_matching_prefix(&first.as_str()[..8])?[0].id(),
            first
        );
        assert_eq!(
            workspace.commits_from_matching_prefix("HEAD", &last.as_str()[..8])?[0].id(),
            last
        );
        Ok(())
    }

    #[test]
    fn endpoint_paths_enumerate_target_without_a_base() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("workspace-endpoint-paths")?;
        init(&dir.0)?;
        commit_and_stage(&dir.0, &[("committed.md", "committed\n")])?;
        let commit = head(&dir.0)?;
        stage(
            &dir.0,
            &[("committed.md", "committed\n"), ("indexed.md", "indexed\n")],
        )?;
        fs::write(dir.0.join("committed.md"), "committed\n")?;
        fs::write(dir.0.join("indexed.md"), "indexed\n")?;
        fs::write(dir.0.join("working.md"), "working\n")?;
        let mut workspace = Workspace::discover(&dir.0)?;

        assert_eq!(
            workspace.endpoint_paths(&ComparisonEndpoint::EmptyTree)?,
            Vec::<PathBuf>::new()
        );
        assert_eq!(
            workspace.endpoint_paths(&ComparisonEndpoint::Commit(commit.clone()))?,
            [PathBuf::from("committed.md")]
        );
        assert_eq!(
            workspace.endpoint_paths(&ComparisonEndpoint::Index)?,
            [PathBuf::from("committed.md"), PathBuf::from("indexed.md")]
        );
        assert_eq!(
            workspace.endpoint_paths(&ComparisonEndpoint::WorkingTree)?,
            [
                PathBuf::from("committed.md"),
                PathBuf::from("indexed.md"),
                PathBuf::from("working.md")
            ]
        );
        assert_eq!(
            workspace.endpoint_text(
                &ComparisonEndpoint::Commit(commit),
                Path::new("committed.md")
            )?,
            Some("committed\n".to_owned())
        );

        let unavailable = ComparisonEndpoint::Commit(CommitId::parse(
            "0000000000000000000000000000000000000000",
        )?);
        assert!(
            workspace
                .endpoint_paths(&unavailable)
                .is_err_and(|error| error.message().contains("cannot read commit"))
        );
        assert!(
            workspace
                .endpoint_paths(&ComparisonEndpoint::ReviewPoint("point-1".to_owned()))
                .is_err_and(|error| error.message().contains("requires ReviewPointStore"))
        );
        Ok(())
    }

    #[test]
    fn working_tree_endpoint_does_not_read_head_objects() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = TempDir::new("workspace-working-without-head-object")?;
        init(&dir.0)?;
        commit_and_stage(&dir.0, &[("tracked.md", "tracked\n")])?;
        fs::write(dir.0.join("tracked.md"), "working\n")?;
        fs::write(dir.0.join("untracked.md"), "untracked\n")?;
        let mut workspace = Workspace::discover(&dir.0)?;

        fs::write(dir.0.join(".git/HEAD"), format!("{}\n", "0".repeat(40)))?;

        assert_eq!(
            workspace.endpoint_paths(&ComparisonEndpoint::WorkingTree)?,
            [PathBuf::from("tracked.md"), PathBuf::from("untracked.md")]
        );
        assert_eq!(
            workspace.endpoint_text(&ComparisonEndpoint::WorkingTree, Path::new("tracked.md"))?,
            Some("working\n".to_owned())
        );
        Ok(())
    }

    #[test]
    fn working_tree_is_final_and_index_remains_explicit() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = TempDir::new("workspace-working-final")?;
        init(&dir.0)?;
        commit_and_stage(&dir.0, &[("a.md", "base\n"), ("gone.md", "gone\n")])?;
        let commit = head(&dir.0)?;
        fs::write(dir.0.join("a.md"), "base\n")?;
        fs::write(dir.0.join("gone.md"), "gone\n")?;
        stage(&dir.0, &[("a.md", "staged\n")])?;
        fs::write(dir.0.join("a.md"), "base\n")?;
        fs::remove_file(dir.0.join("gone.md"))?;
        fs::write(dir.0.join("added.md"), "added\n")?;
        let mut workspace = Workspace::discover(&dir.0)?;

        let net = workspace.compare(
            ComparisonEndpoint::Commit(commit.clone()),
            ComparisonEndpoint::WorkingTree,
        )?;
        let net_paths: Vec<_> = net
            .changes()
            .iter()
            .map(crate::diff::PathChange::path)
            .collect();
        assert_eq!(net_paths, [Path::new("added.md"), Path::new("gone.md")]);
        assert_eq!(net.changes()[0].kind(), crate::diff::PathChangeKind::Added);
        assert_eq!(
            net.changes()[1].kind(),
            crate::diff::PathChangeKind::Deleted
        );

        let staged = workspace.compare(
            ComparisonEndpoint::Commit(commit),
            ComparisonEndpoint::Index,
        )?;
        assert_eq!(staged.changes().len(), 2);
        assert_eq!(staged.changes()[0].path(), Path::new("a.md"));
        assert_eq!(staged.changes()[1].path(), Path::new("gone.md"));
        assert_eq!(
            workspace
                .status()?
                .get(Path::new("a.md"))
                .map(crate::status::Entry::state),
            Some(State::Modified)
        );
        Ok(())
    }

    #[test]
    fn checked_comparison_walk_refuses_an_incomplete_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("workspace-checked-walk")?;
        let mut workspace = Workspace::discover(&dir.0)?;
        let error = workspace
            .walk_files_checked_under(Path::new("missing"), Filter::Visible)
            .err()
            .ok_or("missing directory was silently skipped")?;
        assert!(error.to_string().contains("missing"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn checked_comparison_walk_refuses_an_unrepresentable_entry()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let dir = TempDir::new("workspace-checked-walk-entry")?;
        fs::write(dir.0.join(OsString::from_vec(vec![0xff])), "content")?;
        let mut workspace = Workspace::discover(&dir.0)?;
        let error = workspace
            .walk_files_checked(Filter::Visible)
            .err()
            .ok_or("unrepresentable entry was silently skipped")?;
        assert!(error.to_string().contains("UTF-8"));
        Ok(())
    }
}
