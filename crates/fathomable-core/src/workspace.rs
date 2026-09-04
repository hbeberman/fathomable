// @okf-doc: /decisions/0012-workspace-mode.md
//! The workspace: its root, root-relative paths, and git ignore rules.
//!
//! A [`Workspace`] is rooted at the enclosing git work tree of the path it
//! was opened on, or at that directory itself when there is no repository
//! (ADR 0009). Directory listings hide the `.git` directory and anything git
//! ignores, using the repository's own exclude stack, so the sidebar and the
//! picker agree with `git status` on what exists.
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
use gix::worktree::stack::state::attributes::Source as AttrSource;
use gix::worktree::stack::state::ignore::Source;

use crate::content::{self, Attr};
use crate::diff::Diff;
use crate::status::{self, State, Status};

/// One directory entry, as the sidebar shows it.
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

/// One commit that changed a file, from [`Workspace::file_history`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    hex: String,
    time: u64,
    subject: String,
}

impl Commit {
    /// The full commit id as hex.
    #[must_use]
    pub fn hex(&self) -> &str {
        &self.hex
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
}

/// A workspace root with git ignore evaluation.
pub struct Workspace {
    root: PathBuf,
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
                    tracing::info!(root = %root.display(), "workspace is a git work tree");
                    Ok(Self {
                        root,
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
        Self { root, ignore: None }
    }

    /// The absolute workspace root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
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

    /// Which of `wanted` (hex commits) are `HEAD` or one of its ancestors
    /// (ADR 0024). One walk from `HEAD` answers the whole set; it stops as
    /// soon as every wanted commit has been met. Returns `None` outside git
    /// or before the first commit, meaning nothing can be scoped.
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
        let walk = match git.repo.rev_walk([head]).all() {
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
        let Some(bytes) = self.head_blob(relative)? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| WorkspaceError {
                path: self.root.join(relative),
                message: format!("HEAD blob is not UTF-8 text: {error}"),
            })
    }

    /// The bytes of root-relative `relative` as committed in `HEAD`;
    /// see [`Self::head_text`] for the `None` and empty cases.
    fn head_blob(&self, relative: &Path) -> Result<Option<Vec<u8>>, WorkspaceError> {
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

    /// The text of root-relative `relative` as committed in `commit` (hex),
    /// a side of the checkpoint diff (ADR 0049).
    ///
    /// Returns `None` outside git, and `Some("")` for a file the commit
    /// does not have.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the commit or the blob cannot be
    /// read, or the blob is not UTF-8 text.
    pub fn text_at(&self, commit: &str, relative: &Path) -> Result<Option<String>, WorkspaceError> {
        let Some(git) = self.ignore.as_ref() else {
            return Ok(None);
        };
        let fail = |message: String| WorkspaceError {
            path: self.root.join(relative),
            message,
        };
        let id = ObjectId::from_hex(commit.as_bytes())
            .map_err(|error| fail(format!("not a commit id: {error}")))?;
        let tree = git
            .repo
            .find_commit(id)
            .map_err(|error| fail(format!("cannot read commit {commit}: {error}")))?
            .tree()
            .map_err(|error| fail(format!("cannot read the tree of {commit}: {error}")))?;
        let short = &commit[..commit.len().min(7)];
        let bytes = self.blob_in(&tree, relative, short)?;
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| fail(format!("blob at {short} is not UTF-8 text: {error}")))
    }

    /// The commits reachable from `HEAD` that changed root-relative
    /// `relative`, newest first, at most `limit` of them (ADR 0049). A
    /// commit changed the file when its blob differs from the one in its
    /// first parent, or the file is new there. Empty outside git or before
    /// the first commit.
    #[must_use]
    pub fn file_history(&self, relative: &Path, limit: usize) -> Vec<Commit> {
        let Some(git) = self.ignore.as_ref() else {
            return Vec::new();
        };
        let Ok(head) = git.repo.head_id() else {
            return Vec::new();
        };
        let walk = match git.repo.rev_walk([head]).all() {
            Ok(walk) => walk,
            Err(error) => {
                tracing::warn!(%error, "cannot walk history from HEAD");
                return Vec::new();
            }
        };
        let blob_of = |id: ObjectId| -> Option<Option<ObjectId>> {
            let tree = git.repo.find_commit(id).ok()?.tree().ok()?;
            let entry = tree.lookup_entry_by_path(relative).ok()?;
            Some(entry.map(|entry| entry.oid().to_owned()))
        };
        let mut out = Vec::new();
        for info in walk {
            if out.len() >= limit {
                break;
            }
            let info = match info {
                Ok(info) => info,
                Err(error) => {
                    tracing::warn!(%error, "history walk stopped early");
                    break;
                }
            };
            let id = info.id;
            let Some(blob) = blob_of(id) else {
                continue;
            };
            let parent_blob = info
                .parent_ids()
                .next()
                .map_or(Some(None), |parent| blob_of(parent.detach()));
            let changed = match (blob, parent_blob) {
                (None, _) => false,
                (Some(_), None) => true,
                (Some(now), Some(before)) => Some(now) != before,
            };
            if !changed {
                continue;
            }
            let Ok(commit) = git.repo.find_commit(id) else {
                continue;
            };
            let time = commit
                .time()
                .map_or(0, |time| u64::try_from(time.seconds).unwrap_or(0));
            let subject = commit
                .message_raw_sloppy()
                .lines()
                .next()
                .map(|line| line.to_str_lossy().into_owned())
                .unwrap_or_default();
            out.push(Commit {
                hex: id.to_hex().to_string(),
                time,
                subject,
            });
        }
        out
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
            return Ok(Some(String::new()));
        };
        if !matches!(
            entry.mode,
            gix::index::entry::Mode::FILE
                | gix::index::entry::Mode::FILE_EXECUTABLE
                | gix::index::entry::Mode::SYMLINK
        ) {
            return Ok(Some(String::new()));
        }
        let object = git
            .repo
            .find_object(entry.id)
            .map_err(|error| fail(format!("cannot read blob from the index: {error}")))?;
        String::from_utf8(object.detach().data)
            .map(Some)
            .map_err(|error| fail(format!("index blob is not UTF-8 text: {error}")))
    }

    /// Every uncommitted path (ADR 0017): the index against `HEAD` for
    /// staged changes, the working tree against the index for unstaged
    /// ones, and non-ignored files the index lacks as untracked. Empty
    /// outside git.
    ///
    /// A tracked file whose size and mtime match the index is taken as
    /// clean without reading it, as git does; anything else is hashed.
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
        let unborn = git
            .repo
            .head()
            .map_err(|error| fail(format!("cannot read HEAD: {error}")))?
            .is_unborn();
        if !unborn {
            let tree = git
                .repo
                .head_tree()
                .map_err(|error| fail(format!("cannot read HEAD tree: {error}")))?;
            collect_blobs(&tree, &mut BString::default(), &mut head)
                .map_err(|error| fail(format!("cannot walk HEAD tree: {error}")))?;
        }
        let hash = git.repo.object_hash();
        let mut dirty: BTreeMap<BString, (State, bool)> = BTreeMap::new();
        let mut in_index: BTreeSet<BString> = BTreeSet::new();
        for entry in index.entries() {
            let is_link = entry.mode == gix::index::entry::Mode::SYMLINK;
            if entry.stage() != gix::index::entry::Stage::Unconflicted
                || !(is_link
                    || matches!(
                        entry.mode,
                        gix::index::entry::Mode::FILE | gix::index::entry::Mode::FILE_EXECUTABLE
                    ))
            {
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
            let worktree = worktree_state(&absolute, entry, hash);
            if let Some(state) = worktree.or(staged) {
                dirty.insert(path, (state, staged.is_some()));
            }
        }
        for path in head.keys() {
            if !in_index.contains(path) {
                dirty.insert(path.clone(), (State::Deleted, true));
            }
        }
        for file in self.walk_files(Filter::Visible) {
            let path = BString::from(file);
            if !in_index.contains(&path) {
                dirty.insert(path, (State::Untracked, false));
            }
        }
        let mut entries = Vec::with_capacity(dirty.len());
        for (path, (state, staged)) in dirty {
            let relative = gix::path::from_bstr(path.as_bstr()).into_owned();
            let entry = match self.count_lines(&relative, state) {
                Lines::Text { added, removed } => {
                    status::Entry::new(relative, state, added, removed)
                }
                Lines::Binary => status::Entry::new(relative, state, 0, 0).binary(),
            };
            entries.push(if staged { entry.staged() } else { entry });
        }
        let status = Status::from_entries(entries);
        tracing::debug!(dirty = status.len(), elapsed = ?started.elapsed(), "git status");
        Ok(status)
    }

    /// Line counts of the working tree against `HEAD`, or that the file
    /// is binary by git's rule (ADR 0026): the `diff` attribute, else a
    /// `NUL` in the first bytes of whichever side exists.
    fn count_lines(&mut self, relative: &Path, state: State) -> Lines {
        let attr = self.diff_attr(relative);
        let head = match state {
            State::Untracked => Some(Vec::new()),
            _ => self.head_blob(relative).ok().flatten(),
        };
        let worktree = match state {
            State::Deleted => Some(Vec::new()),
            _ => self.worktree_bytes(relative),
        };
        let (Some(old), Some(new)) = (head, worktree) else {
            return Lines::Text {
                added: 0,
                removed: 0,
            };
        };
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
    pub fn list_dir_with(
        &mut self,
        relative: &Path,
        filter: Filter,
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
                Err(error) => {
                    tracing::warn!(%error, dir = %dir.display(), "skipping unreadable entry");
                    continue;
                }
            };
            let name = item.file_name();
            let Some(name) = name.to_str().map(str::to_owned) else {
                tracing::debug!(dir = %dir.display(), "skipping non-UTF-8 file name");
                continue;
            };
            if name == ".git" {
                continue;
            }
            // Browsing may follow a symlink into its target directory,
            // but git never does: for ignore rules and the dirty set the
            // link is a file-like entry, wherever it points.
            let file_type = item.file_type().ok();
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
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => a
                .name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name)),
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
        let mut out = Vec::new();
        let mut pending = vec![PathBuf::new()];
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
                    out.push(path.to_string_lossy().into_owned());
                }
            }
            // Push in reverse so the stack yields subdirectories in order.
            pending.extend(dirs.into_iter().rev());
        }
        out
    }
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

/// Every blob under `tree`, keyed by slash-separated path.
/// How the working tree differs from index `entry` at `absolute`: `None`
/// when they match, otherwise the [`State`] the entry is in.
///
/// A tracked file whose size and mtime match the index is taken as clean
/// without reading it, as git does; anything else is hashed. A symlink's
/// blob is its target path, so that is what gets hashed, never the file
/// the link points at.
fn worktree_state(
    absolute: &Path,
    entry: &gix::index::Entry,
    hash: gix::hash::Kind,
) -> Option<State> {
    let is_link = entry.mode == gix::index::entry::Mode::SYMLINK;
    match gix::index::fs::Metadata::from_path_no_follow(absolute) {
        Ok(meta) if meta.is_file() && !is_link => {
            let fresh = gix::index::entry::Stat::from_fs(&meta)
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
