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

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gix::ObjectId;
use gix::bstr::{BString, ByteSlice};
use gix::revision::walk::Sorting;
use gix::traverse::commit::simple::CommitTimeOrder;
use gix::worktree::stack::state::attributes::Source as AttrSource;
use gix::worktree::stack::state::ignore::Source;

use crate::content::{self, Attr};
use crate::diff::{Comparison, Diff, FileMode, PathChange, PathInfo, PathState};
use crate::status::{self, Changes, State, Status};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listing {
    BestEffort,
    Complete,
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

/// A contiguous first-parent commit selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitBatch {
    before: ComparisonEndpoint,
    after: ComparisonEndpoint,
    commits: Vec<CommitId>,
}

impl CommitBatch {
    /// The endpoint immediately before the first selected commit.
    #[must_use]
    pub const fn before(&self) -> &ComparisonEndpoint {
        &self.before
    }

    /// The last selected commit endpoint.
    #[must_use]
    pub const fn after(&self) -> &ComparisonEndpoint {
        &self.after
    }

    /// Selected commits in chronological order.
    #[must_use]
    pub fn commits(&self) -> &[CommitId] {
        &self.commits
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
    /// What the state directory is keyed by (ADR 0070): the canonical
    /// git common dir, or the root outside git.
    key: PathBuf,
    ignore: Option<Ignore>,
}

struct Ignore {
    repo: gix::Repository,
    /// Ignore rules and attributes together, one stack for both queries.
    stack: gix::worktree::Stack,
    /// Reused match scratch for the `diff` attribute (ADR 0026).
    diff_attr: gix::attrs::search::Outcome,
}

impl fmt::Debug for Workspace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Workspace")
            .field("root", &self.root)
            .field("key", &self.key)
            .field("git", &self.ignore.is_some())
            .finish()
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
                    tracing::info!(root = %root.display(), key = %key.display(), "workspace is a git work tree");
                    Ok(Self {
                        root,
                        key,
                        ignore: Some(ignore),
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
            key: root.clone(),
            root,
            ignore: None,
        }
    }

    /// The absolute workspace root: the worktree this instance reads.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
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

    /// Every worktree of the repository (ADR 0070): the main one first,
    /// then the linked ones as git keeps them, or nothing outside git.
    #[must_use]
    pub fn worktrees(&self) -> Vec<crate::worktrees::Worktree> {
        self.ignore
            .as_ref()
            .map(|git| crate::worktrees::list(&git.repo))
            .unwrap_or_default()
    }

    /// The paths a viewer watches for the worktree set and the other
    /// worktrees' `HEAD`s moving (ADR 0070): the common dir, its `refs`,
    /// its `worktrees/` registry, and each linked worktree's git dir;
    /// none outside git.
    #[must_use]
    pub fn worktree_watch_paths(&self) -> Vec<PathBuf> {
        let Some(git) = self.ignore.as_ref() else {
            return Vec::new();
        };
        let common = crate::worktrees::canonical(git.repo.common_dir());
        let registry = crate::worktrees::registry(&common);
        let mut out = vec![common.clone(), common.join("refs")];
        match std::fs::read_dir(&registry) {
            Ok(entries) => {
                out.push(registry);
                out.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => out.push(registry),
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

    /// Compute the direct first-parent boundary for a contiguous commit batch.
    ///
    /// The returned pair is the tree before the first selected commit and the
    /// last selected commit. No merge base or three-dot comparison is used.
    /// If the first selected commit is a merge, the boundary is ambiguous and
    /// an actionable error names its parents.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when either commit is unavailable, the
    /// first is not an ancestor on the last commit's first-parent chain, or
    /// the first commit has multiple possible parents.
    pub fn contiguous_batch(
        &self,
        first: &CommitId,
        last: &CommitId,
    ) -> Result<CommitBatch, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Err(WorkspaceError {
                path: self.root.clone(),
                message: "contiguous commit batches require a Git repository".to_owned(),
            });
        };
        let first_id = ObjectId::from_hex(first.as_str().as_bytes()).map_err(|error| {
            self.revision_error(first.as_str(), &format!("invalid first commit: {error}"))
        })?;
        let last_id = ObjectId::from_hex(last.as_str().as_bytes()).map_err(|error| {
            self.revision_error(last.as_str(), &format!("invalid last commit: {error}"))
        })?;
        let mut chain = Vec::new();
        let mut current = last_id;
        loop {
            let commit = git
                .repo
                .find_commit(current)
                .map_err(|error| WorkspaceError {
                    path: self.root.clone(),
                    message: format!("cannot read commit {}: {error}", current.to_hex()),
                })?;
            let parents: Vec<ObjectId> = commit.parent_ids().map(gix::Id::detach).collect();
            if parents.len() > 1 {
                let listed = parents
                    .iter()
                    .map(|parent| parent.to_hex().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(WorkspaceError {
                    path: self.root.clone(),
                    message: format!(
                        "commit {} is a merge; choose an explicit parent boundary ({listed})",
                        current.to_hex()
                    ),
                });
            }
            chain.push(current);
            if current == first_id {
                break;
            }
            let Some(parent) = parents.first().copied() else {
                return Err(WorkspaceError {
                    path: self.root.clone(),
                    message: format!(
                        "commit {} is not an ancestor of {} on the first-parent chain",
                        first.short(),
                        last.short()
                    ),
                });
            };
            current = parent;
        }
        let first_commit = git
            .repo
            .find_commit(first_id)
            .map_err(|error| WorkspaceError {
                path: self.root.clone(),
                message: format!("cannot read first commit {}: {error}", first.short()),
            })?;
        let parents: Vec<ObjectId> = first_commit.parent_ids().map(gix::Id::detach).collect();
        let before = match parents.as_slice() {
            [] => ComparisonEndpoint::EmptyTree,
            [parent] => ComparisonEndpoint::Commit(CommitId(parent.to_hex().to_string())),
            _ => {
                let listed = parents
                    .iter()
                    .map(|parent| parent.to_hex().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(WorkspaceError {
                    path: self.root.clone(),
                    message: format!(
                        "commit {} is a merge; choose an explicit parent boundary ({listed})",
                        first.short()
                    ),
                });
            }
        };
        chain.reverse();
        Ok(CommitBatch {
            before,
            after: ComparisonEndpoint::Commit(last.clone()),
            commits: chain
                .into_iter()
                .map(|id| CommitId(id.to_hex().to_string()))
                .collect(),
        })
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
        let base_files = self.endpoint_files(&base)?;
        let target_files = self.endpoint_files(&target)?;
        let mut paths = BTreeSet::new();
        paths.extend(base_files.keys().cloned());
        paths.extend(target_files.keys().cloned());
        let mut changes = Vec::new();
        for path in paths {
            let base_file = base_files.get(&path);
            let target_file = target_files.get(&path);
            let mut base_state = base_file.map_or(PathState::Absent, |file| {
                PathState::Present(file.info.clone())
            });
            let mut target_state = target_file.map_or(PathState::Absent, |file| {
                PathState::Present(file.info.clone())
            });
            let base_bytes = if base_file.is_some_and(|file| file.info.is_supported()) {
                match self.endpoint_bytes(&base, &path) {
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
                match self.endpoint_bytes(&target, &path) {
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
        Ok(Comparison::from_parts(base, target, changes))
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
        match endpoint {
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
        }
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
        let object = entry.object().map_err(|error| WorkspaceError {
            path: self.root.join(relative),
            message: format!(
                "cannot read blob for {} at commit {}: {error}",
                relative.display(),
                id.short()
            ),
        })?;
        Ok(Some(object.detach().data))
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
            return fs::read(&absolute)
                .map(Some)
                .map_err(|error| WorkspaceError {
                    path: absolute,
                    message: format!("cannot read working-tree file: {error}"),
                });
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
        collect_endpoint_tree(&git.repo, &tree, Path::new(""), &mut files).map_err(|message| {
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
        let mut files = BTreeMap::new();
        for relative in self.walk_files_checked(Filter::All)? {
            if self.is_ignored(&relative, EntryKind::File) && !self.is_tracked_path(&relative) {
                continue;
            }
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
            }
            files.insert(
                relative,
                EndpointFile {
                    info: PathInfo::new(mode, size, None, false)
                        .with_supported(endpoint_size_supported(mode, size)),
                },
            );
        }
        Ok(files)
    }

    fn is_tracked_path(&self, relative: &Path) -> bool {
        let Some(git) = self.ignore.as_ref() else {
            return false;
        };
        if let Ok(index) = git.repo.index_or_empty()
            && index.entry_by_path(unix_path(relative).as_ref()).is_some()
        {
            return true;
        }
        git.repo
            .head_tree()
            .ok()
            .and_then(|tree| tree.lookup_entry_by_path(relative).ok().flatten())
            .is_some()
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
    /// diff base for the gutter and the diff view (ADR 0006).
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
            tracing::debug!("HEAD is unborn; diff base is empty");
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
        let mut head: BTreeMap<BString, ObjectId> = BTreeMap::new();
        if let Some(tree) = head_tree_of(&git.repo).map_err(&fail)? {
            collect_blobs(&tree, &mut BString::default(), &mut head)
                .map_err(|error| fail(format!("cannot walk HEAD tree: {error}")))?;
        }
        let hash = git.repo.object_hash();
        let mut dirty: BTreeMap<BString, (Option<State>, Option<State>)> = BTreeMap::new();
        let mut in_index: BTreeSet<BString> = BTreeSet::new();
        for entry in index.entries() {
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
            let absolute = self.root.join(gix::path::from_bstr(path.as_bstr()));
            let worktree = worktree_state(&absolute, entry, hash, &index);
            if Changes::from_sides(staged, worktree).is_some() {
                dirty.insert(path, (staged, worktree));
            }
        }
        for path in head.keys() {
            if !in_index.contains(path) {
                dirty.insert(path.clone(), (Some(State::Deleted), None));
            }
        }
        for file in self.walk_files(Filter::Visible) {
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
                entries.push(self.dirty_entry(relative, changes));
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
        let mut head = Head {
            tree: head_tree_of(&repo).map_err(&fail)?,
            hash: repo.object_hash(),
            collected: BTreeMap::new(),
            covered: Vec::new(),
        };
        let examine = self.paths_to_examine(&index, previous, changed, &mut head)?;
        let mut entries: Vec<status::Entry> = previous
            .entries()
            .iter()
            .filter(|entry| !examine.contains(entry.path()))
            .cloned()
            .collect();
        for relative in &examine {
            if let Some(changes) = self.dirty_state(&index, &head, previous, relative)? {
                entries.push(self.dirty_entry(relative.clone(), changes));
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
            }
            for entry in previous.entries() {
                if entry.path().starts_with(path) {
                    examine.insert(entry.path().to_path_buf());
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
                self.walk_under(path, Filter::Visible, &mut examine);
            }
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
                worktree_state(&absolute, entry, head.hash, index),
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
    fn dirty_entry(&mut self, relative: PathBuf, changes: Changes) -> status::Entry {
        match self.count_lines(&relative) {
            Lines::Text { added, removed } => status::Entry::new(relative, changes, added, removed),
            Lines::Binary => status::Entry::new(relative, changes, 0, 0).binary(),
        }
    }

    /// Line counts of the working tree against `HEAD`, or that the file
    /// is binary by git's rule (ADR 0026): the `diff` attribute, else a
    /// `NUL` in the first bytes of whichever side exists.
    fn count_lines(&mut self, relative: &Path) -> Lines {
        let attr = self.diff_attr(relative);
        let old = self.head_bytes(relative).ok().flatten().unwrap_or_default();
        let new = self.worktree_bytes(relative).unwrap_or_default();
        let binary = attr
            .decided()
            .unwrap_or_else(|| content::is_binary(&old) || content::is_binary(&new));
        if binary {
            return Lines::Binary;
        }
        match (String::from_utf8(old), String::from_utf8(new)) {
            (Ok(old), Ok(new)) => {
                let (added, removed) = Diff::new(&old, &new).counts();
                Lines::Text { added, removed }
            }
            _ => Lines::Text {
                added: 0,
                removed: 0,
            },
        }
    }

    /// The diffable bytes of `relative` on disk: a symlink's target path,
    /// matching the blob git stores for it, or the file's content.
    fn worktree_bytes(&self, relative: &Path) -> Option<Vec<u8>> {
        let absolute = self.root.join(relative);
        match absolute.symlink_metadata() {
            Ok(meta) if meta.is_symlink() => fs::read_link(&absolute)
                .ok()
                .map(|target| target.to_string_lossy().into_owned().into_bytes()),
            _ => fs::read(&absolute).ok(),
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
        self.list_dir_with_policy(relative, filter, Listing::BestEffort)
    }

    fn list_dir_checked(
        &mut self,
        relative: &Path,
        filter: Filter,
    ) -> Result<Vec<Entry>, WorkspaceError> {
        self.list_dir_with_policy(relative, filter, Listing::Complete)
    }

    fn list_dir_with_policy(
        &mut self,
        relative: &Path,
        filter: Filter,
        listing: Listing,
    ) -> Result<Vec<Entry>, WorkspaceError> {
        let dir = self.root.join(relative);
        let read = fs::read_dir(&dir).map_err(|source| WorkspaceError {
            path: dir.clone(),
            message: format!("cannot read directory: {source}"),
        })?;
        let mut entries = Vec::new();
        for item in read {
            let item = match item {
                Ok(item) => item,
                Err(error) if listing == Listing::Complete => {
                    return Err(WorkspaceError {
                        path: dir,
                        message: format!("cannot enumerate directory: {error}"),
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, dir = %dir.display(), "skipping unreadable entry");
                    continue;
                }
            };
            let name = item.file_name();
            let name = match name.to_str() {
                Some(name) => name.to_owned(),
                None if listing == Listing::Complete => {
                    return Err(WorkspaceError {
                        path: item.path(),
                        message: "file name is not valid UTF-8".to_owned(),
                    });
                }
                None => {
                    tracing::debug!(dir = %dir.display(), "skipping non-UTF-8 file name");
                    continue;
                }
            };
            if name == ".git" {
                continue;
            }
            // Browsing may follow a symlink into its target directory,
            // but git never does: for ignore rules and the dirty set the
            // link is a file-like entry, wherever it points.
            let file_type = match item.file_type() {
                Ok(file_type) => Some(file_type),
                Err(error) if listing == Listing::Complete => {
                    return Err(WorkspaceError {
                        path: item.path(),
                        message: format!("cannot inspect directory entry: {error}"),
                    });
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        path = %item.path().display(),
                        "cannot inspect directory entry"
                    );
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
        entries.sort_by(|a, b| entry_order(a.is_dir, &a.name, b.is_dir, &b.name));
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
        let mut files = Vec::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            let entries = self.list_dir_checked(&dir, filter)?;
            let mut dirs = Vec::new();
            for entry in entries {
                let path = dir.join(&entry.name);
                if entry.is_dir && !entry.is_link {
                    dirs.push(path);
                } else {
                    files.push(path);
                }
            }
            pending.extend(dirs.into_iter().rev());
        }
        Ok(files)
    }

    /// The files under root-relative `dir` as [`Workspace::walk_files`]
    /// lists them, `dir` itself listed first: what a directory that
    /// arrived whole adds to an index kept in [`walk_order`].
    pub fn walk_files_under(&mut self, dir: &Path, filter: Filter) -> Vec<String> {
        let mut files: Vec<PathBuf> = Vec::new();
        self.walk_under(dir, filter, &mut files);
        files
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }

    /// The walk of [`Workspace::walk_files`] from root-relative `dir`
    /// down, each file pushed onto `out`.
    fn walk_under<C>(&mut self, dir: &Path, filter: Filter, out: &mut C)
    where
        C: Extend<PathBuf>,
    {
        let mut pending = vec![dir.to_path_buf()];
        while let Some(dir) = pending.pop() {
            let entries = match self.list_dir_with(&dir, filter) {
                Ok(entries) => entries,
                Err(error) => {
                    tracing::warn!(%error, "skipping directory while indexing");
                    continue;
                }
            };
            let mut dirs = Vec::new();
            for entry in entries {
                let path = dir.join(&entry.name);
                if entry.is_dir && !entry.is_link {
                    dirs.push(path);
                } else {
                    out.extend(std::iter::once(path));
                }
            }
            // Push in reverse so the stack yields subdirectories in order.
            pending.extend(dirs.into_iter().rev());
        }
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
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &Path,
    out: &mut BTreeMap<PathBuf, EndpointFile>,
) -> Result<(), String> {
    for entry in tree.iter() {
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
            let subtree = entry
                .object()
                .map_err(|error| error.to_string())?
                .into_tree();
            collect_endpoint_tree(repo, &subtree, &path, out)?;
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
pub fn open_options() -> gix::open::Options {
    let mut permissions = gix::open::Permissions::default();
    permissions.env.git_prefix = gix::sec::Permission::Deny;
    gix::open::Options::default().permissions(permissions)
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
                collect_blobs(&subtree, &mut prefix.clone(), &mut self.collected)
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
    absolute: &Path,
    entry: &gix::index::Entry,
    hash: gix::hash::Kind,
    index: &gix::index::State,
) -> Option<State> {
    let is_link = entry.mode == gix::index::entry::Mode::SYMLINK;
    match gix::index::fs::Metadata::from_path_no_follow(absolute) {
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
                let same = fs::read(absolute).ok().is_some_and(|data| {
                    gix::objs::compute_hash(hash, gix::objs::Kind::Blob, &data)
                        .is_ok_and(|id| id == entry.id)
                });
                (!same).then_some(State::Modified)
            }
        }
        Ok(meta) if meta.is_symlink() && is_link => {
            let same = fs::read_link(absolute).ok().is_some_and(|target| {
                let target = gix::path::to_unix_separators_on_windows(gix::path::into_bstr(
                    target.as_path(),
                ));
                gix::objs::compute_hash(hash, gix::objs::Kind::Blob, target.as_ref())
                    .is_ok_and(|id| id == entry.id)
            });
            (!same).then_some(State::Modified)
        }
        // A file became a symlink or the other way around.
        Ok(meta) if meta.is_file() || meta.is_symlink() => Some(State::Modified),
        _ => Some(State::Deleted),
    }
}

fn collect_blobs(
    tree: &gix::Tree<'_>,
    prefix: &mut BString,
    out: &mut BTreeMap<BString, ObjectId>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for entry in tree.iter() {
        let entry = entry?;
        let len = prefix.len();
        prefix.extend_from_slice(entry.filename());
        if entry.mode().is_tree() {
            let subtree = entry.object()?.into_tree();
            prefix.push(b'/');
            collect_blobs(&subtree, prefix, out)?;
        } else if entry.mode().is_blob_or_symlink() {
            out.insert(prefix.clone(), entry.object_id());
        }
        prefix.truncate(len);
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
    fn recent_head_commits_and_batches_use_explicit_history()
    -> Result<(), Box<dyn std::error::Error>> {
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
        let batch = workspace.contiguous_batch(&first, &last)?;
        assert_eq!(batch.before(), &ComparisonEndpoint::EmptyTree);
        assert_eq!(batch.after(), &ComparisonEndpoint::Commit(last));
        assert_eq!(batch.commits().len(), 3);
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
