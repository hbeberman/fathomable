// @okf-doc: /decisions/0012-workspace-mode.md
//! The tree pane's tree: lazily expanded directories with a cursor.
//!
//! [`Tree`] is plain data over a [`Workspace`]: directories are read when
//! first expanded, and the visible rows are recomputed after every change so
//! the terminal can draw them and the keys can move a cursor over them
//! (ADR 0012). Filesystem access always goes through the workspace so ignore
//! rules apply.
//!
//! What the tree lists is a [`Shown`] (ADR 0068): every file, or only the
//! changed ones, only externally identified review paths, without the
//! untracked ones, with the ignored ones. The rows are filtered as they are
//! built, so the cursor, clicks, and [`Tree::reveal`] see only the listed rows.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use crate::status::{State, Status};
use crate::workspace::{EntryKind, Filter, Workspace, WorkspaceError, entry_order};

/// What the tree lists (ADR 0068): every file, or a subset by four rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shown {
    rules: u8,
}

impl Default for Shown {
    fn default() -> Self {
        Self::all()
    }
}

impl Shown {
    const CHANGED: u8 = 1 << 0;
    const REVIEWS: u8 = 1 << 1;
    const HIDE_UNTRACKED: u8 = 1 << 2;
    const IGNORED: u8 = 1 << 3;

    /// Every non-ignored file: no rule on.
    #[must_use]
    pub const fn all() -> Self {
        Self { rules: 0 }
    }

    /// Whether no rule is on.
    #[must_use]
    pub const fn is_all(self) -> bool {
        self.rules == 0
    }

    /// Only the files that differ from `HEAD` are listed.
    #[must_use]
    pub const fn changed_only(self) -> bool {
        self.rules & Self::CHANGED != 0
    }

    /// Only files in the externally supplied review paths are listed.
    #[must_use]
    pub const fn reviews_only(self) -> bool {
        self.rules & Self::REVIEWS != 0
    }

    /// Untracked files are listed.
    #[must_use]
    pub const fn untracked(self) -> bool {
        self.rules & Self::HIDE_UNTRACKED == 0
    }

    /// Files git ignores are listed.
    #[must_use]
    pub const fn ignored(self) -> bool {
        self.rules & Self::IGNORED != 0
    }

    /// The same rules with `rule` flipped.
    #[must_use]
    pub const fn toggled(self, rule: Rule) -> Self {
        Self {
            rules: self.rules ^ rule.mask(),
        }
    }

    /// The workspace filter the tree reads its listings with.
    const fn filter(self) -> Filter {
        if self.ignored() {
            Filter::All
        } else {
            Filter::Visible
        }
    }
}

/// One of the four rules a [`Shown`] toggles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rule {
    /// Only changed files.
    Changed,
    /// Only files with qualifying reviews.
    Reviews,
    /// Untracked files.
    Untracked,
    /// Ignored files.
    Ignored,
}

impl Rule {
    const fn mask(self) -> u8 {
        match self {
            Self::Changed => Shown::CHANGED,
            Self::Reviews => Shown::REVIEWS,
            Self::Untracked => Shown::HIDE_UNTRACKED,
            Self::Ignored => Shown::IGNORED,
        }
    }
}

/// The paths the rules admit under one status.
#[derive(Debug, Clone, Default)]
struct Admitted {
    files: Option<HashSet<PathBuf>>,
    directories: Option<HashSet<PathBuf>>,
    hide: HashSet<PathBuf>,
}

impl Admitted {
    fn new(
        shown: Shown,
        status: &Status,
        review_paths: &[PathBuf],
        snapshot_paths: Option<&[PathBuf]>,
    ) -> Self {
        let counted = status
            .entries()
            .iter()
            .filter(|entry| shown.untracked() || !entry.changes().is_only_untracked());
        let changed = shown.changed_only().then(|| {
            counted
                .clone()
                .map(|entry| entry.path().to_path_buf())
                .collect()
        });
        let reviews = shown
            .reviews_only()
            .then(|| review_paths.iter().cloned().collect());
        let hide = if shown.untracked() {
            HashSet::new()
        } else {
            status
                .entries()
                .iter()
                .filter(|entry| entry.changes().is_only_untracked())
                .map(|entry| entry.path().to_path_buf())
                .collect()
        };
        let scope = snapshot_paths.map(|paths| paths.iter().cloned().collect::<HashSet<PathBuf>>());
        let mut files: Option<HashSet<PathBuf>> = None;
        for candidates in [changed, reviews, scope].into_iter().flatten() {
            files = Some(match files.take() {
                None => candidates,
                Some(mut files) => {
                    files.retain(|path| candidates.contains(path));
                    files
                }
            });
        }
        let files = files.map(|mut files| {
            files.retain(|path| !hide.contains(path));
            files
        });
        let directories = files.as_ref().map(|files| {
            files
                .iter()
                .flat_map(|path| path.ancestors().skip(1))
                .filter(|path| !path.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .collect()
        });
        Self {
            files,
            directories,
            hide,
        }
    }

    fn admits(&self, path: &Path, is_dir: bool) -> bool {
        let selected = if is_dir {
            &self.directories
        } else {
            &self.files
        };
        selected
            .as_ref()
            .is_none_or(|selected| selected.contains(path))
            && !self.hide.contains(path)
    }
}

/// A visible line of the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    path: PathBuf,
    name: String,
    depth: usize,
    is_dir: bool,
    expanded: bool,
}

impl Row {
    /// Root-relative path of the entry.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The entry's file name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Nesting depth; direct children of the root are depth 0.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Whether the entry is a directory.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    /// Whether a directory entry is currently expanded.
    #[must_use]
    pub fn expanded(&self) -> bool {
        self.expanded
    }
}

/// Counts of the entries directly inside a directory.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DirectoryCounts {
    files: usize,
    subdirectories: usize,
}

impl DirectoryCounts {
    /// Number of files admitted by the tree's current filters.
    #[must_use]
    pub const fn files(self) -> usize {
        self.files
    }

    /// Number of subdirectories admitted by the tree's current filters.
    #[must_use]
    pub const fn subdirectories(self) -> usize {
        self.subdirectories
    }
}

#[derive(Debug, Clone)]
struct Node {
    name: String,
    is_dir: bool,
    /// Recursive traversal treats directory symlinks as files.
    kind: EntryKind,
    expanded: bool,
    /// `None` until first expanded.
    children: Option<Vec<Node>>,
    /// Supplied by git status rather than a directory listing.
    deleted: bool,
}

impl Node {
    fn mark_deleted(&mut self) {
        self.deleted = true;
        if let Some(children) = &mut self.children {
            for child in children {
                child.mark_deleted();
            }
        }
    }
}

/// Deleted files and their ancestors, grouped into directory listings.
#[derive(Debug, Clone, Default)]
struct Deleted {
    children: HashMap<PathBuf, Vec<Node>>,
}

impl Deleted {
    fn new(status: &Status, virtual_paths: &[PathBuf]) -> Self {
        let mut deleted = Self::default();
        let mut paths: Vec<PathBuf> = status
            .entries()
            .iter()
            .filter(|entry| {
                entry.unstaged_state() == Some(State::Deleted)
                    || (entry.staged_state() == Some(State::Deleted)
                        && entry.unstaged_state().is_none())
            })
            .map(|entry| entry.path().to_path_buf())
            .collect();
        paths.extend(virtual_paths.iter().cloned());
        paths.sort();
        paths.dedup();
        for entry_path in paths {
            for path in entry_path.ancestors() {
                let Some((parent, name)) = path
                    .parent()
                    .zip(path.file_name().and_then(|name| name.to_str()))
                else {
                    break;
                };
                let children = deleted.children.entry(parent.to_path_buf()).or_default();
                if children.iter().any(|child| child.name == name) {
                    break;
                }
                children.push(Node {
                    name: name.to_owned(),
                    is_dir: path != entry_path.as_path(),
                    kind: if path == entry_path.as_path() {
                        EntryKind::File
                    } else {
                        EntryKind::Dir
                    },
                    expanded: false,
                    children: None,
                    deleted: true,
                });
            }
        }
        deleted
    }

    fn merge(&self, path: &Path, children: &mut Vec<Node>) {
        let deleted = self.children.get(path).map_or(&[][..], Vec::as_slice);
        children
            .retain(|child| !child.deleted || deleted.iter().any(|entry| entry.name == child.name));
        let mut added = false;
        for entry in deleted {
            match children.iter().position(|child| child.name == entry.name) {
                Some(index) if children[index].is_dir != entry.is_dir => {
                    children[index] = entry.clone();
                    added = true;
                }
                Some(_) => {}
                None => {
                    children.push(entry.clone());
                    added = true;
                }
            }
        }
        if added {
            children.sort_by(|a, b| entry_order(a.is_dir, &a.name, b.is_dir, &b.name));
        }
    }

    fn sync(&self, node: &mut Node, path: &Path) {
        if let Some(children) = node.children.as_mut() {
            self.merge(path, children);
            for child in children {
                self.sync(child, &path.join(&child.name));
            }
        }
    }
}

/// What happened when the user activated the row under the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Activation {
    /// A file to open, root-relative.
    Open(PathBuf),
    /// A directory was expanded or collapsed.
    Toggled,
}

/// The tree over one workspace, shown in the sidebar's tree pane.
#[derive(Debug, Clone)]
pub struct Tree {
    root: Node,
    rows: Vec<Row>,
    cursor: usize,
    shown: Shown,
    admitted: Admitted,
    status: Status,
    deleted: Deleted,
    review_paths: Vec<PathBuf>,
    admitted_review_paths: Vec<PathBuf>,
    virtual_paths: Vec<PathBuf>,
    snapshot_paths: Option<Vec<PathBuf>>,
}

impl Tree {
    /// Build a tree with the root expanded one level.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the root cannot be read.
    pub fn new(workspace: &mut Workspace) -> Result<Self, WorkspaceError> {
        let mut tree = Self {
            root: Node {
                name: String::new(),
                is_dir: true,
                kind: EntryKind::Dir,
                expanded: true,
                children: None,
                deleted: false,
            },
            rows: Vec::new(),
            cursor: 0,
            shown: Shown::all(),
            admitted: Admitted::default(),
            status: Status::default(),
            deleted: Deleted::default(),
            review_paths: Vec::new(),
            admitted_review_paths: Vec::new(),
            virtual_paths: Vec::new(),
            snapshot_paths: None,
        };
        tree.refresh(workspace)?;
        Ok(tree)
    }

    /// What the tree lists (ADR 0068).
    #[must_use]
    pub fn shown(&self) -> Shown {
        self.shown
    }

    /// List by `shown` under `status`, keeping the cursor's path where it
    /// is still listed. The listings are re-read when the ignored rule
    /// flips, since that changes what a directory holds.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the root cannot be re-read.
    pub fn set_shown(
        &mut self,
        workspace: &mut Workspace,
        status: &Status,
        shown: Shown,
    ) -> Result<(), WorkspaceError> {
        let reread = shown.ignored() != self.shown.ignored();
        self.shown = shown;
        self.sync_review_paths(workspace);
        if reread {
            self.refresh(workspace)?;
        }
        self.sift(status);
        Ok(())
    }

    /// Re-apply the rules to a new `status`: a file that became clean
    /// leaves an only-changed listing, one that changed appears. Deleted
    /// files remain listed, together with any missing parent directories.
    pub fn sift(&mut self, status: &Status) {
        self.status.clone_from(status);
        self.admitted = Admitted::new(
            self.shown,
            &self.status,
            &self.admitted_review_paths,
            self.snapshot_paths.as_deref(),
        );
        self.deleted = Deleted::new(status, &self.virtual_paths);
        let cursor_path = self.current().map(|row| row.path.clone());
        self.rebuild();
        if let Some(path) = cursor_path {
            self.select_path(&path);
        }
    }

    /// Supply the file paths eligible for the reviews-only rule.
    ///
    /// Paths are root-relative values projected by the caller. Their
    /// ancestors are retained as directories only while they lead to an
    /// eligible file. Ignored paths qualify only while ignored files are
    /// shown. Updating the paths re-applies admission when the rule is active
    /// and otherwise defers work until it is enabled.
    pub fn set_review_paths(
        &mut self,
        workspace: &mut Workspace,
        status: &Status,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        let mut paths: Vec<_> = paths.into_iter().collect();
        paths.sort();
        paths.dedup();
        let paths_changed = paths != self.review_paths;
        self.review_paths = paths;
        let admitted_before = self.admitted_review_paths.clone();
        self.sync_review_paths(workspace);
        if self.shown.reviews_only()
            && (paths_changed || admitted_before != self.admitted_review_paths)
        {
            self.sift(status);
        }
    }

    fn sync_review_paths(&mut self, workspace: &mut Workspace) {
        if self.shown.ignored() {
            self.admitted_review_paths.clone_from(&self.review_paths);
            return;
        }
        self.admitted_review_paths.clear();
        for path in &self.review_paths {
            if !workspace.is_ignored(path, EntryKind::File) {
                self.admitted_review_paths.push(path.clone());
            }
        }
    }

    /// Retain selected-comparison paths absent from the active checkout.
    ///
    /// These virtual entries use the tree's missing-entry channel for
    /// navigation, while the viewer supplies their comparison-specific
    /// addition/deletion presentation.
    pub fn set_virtual_paths(&mut self, status: &Status, paths: Vec<PathBuf>) {
        self.snapshot_paths = None;
        self.virtual_paths = paths;
        self.virtual_paths.sort();
        self.virtual_paths.dedup();
        self.sift(status);
    }

    /// List only one endpoint snapshot and comparison deletions.
    ///
    /// Every supplied path is treated as a file-like entry. Its ancestors
    /// become directories, including when neither exists in the checkout.
    pub fn set_snapshot_paths(&mut self, status: &Status, paths: Vec<PathBuf>) {
        self.virtual_paths = paths;
        self.virtual_paths.sort();
        self.virtual_paths.dedup();
        self.snapshot_paths = Some(self.virtual_paths.clone());
        self.sift(status);
    }

    /// The visible rows, top to bottom.
    #[must_use]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Index into [`Tree::rows`] of the cursor row.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The row under the cursor, if the tree is not empty.
    #[must_use]
    pub fn current(&self) -> Option<&Row> {
        self.rows.get(self.cursor)
    }

    /// Count the visible entries directly inside the selected directory,
    /// reading its listing without expanding it.
    ///
    /// Returns `None` when the selected row is not a directory.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the directory cannot be read.
    pub fn current_directory_counts(
        &mut self,
        workspace: &mut Workspace,
    ) -> Result<Option<DirectoryCounts>, WorkspaceError> {
        let Some(path) = self
            .current()
            .filter(|row| row.is_dir)
            .map(|row| row.path.clone())
        else {
            return Ok(None);
        };
        let filter = self.shown.filter();
        let Some(node) = find_node(&mut self.root, &path) else {
            return Ok(None);
        };
        if node.children.is_none() {
            node.children = Some(read_children(workspace, &path, filter, &self.deleted)?);
        }
        let counts = node
            .children
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|child| self.admitted.admits(&path.join(&child.name), child.is_dir))
            .fold(DirectoryCounts::default(), |mut counts, child| {
                if child.is_dir {
                    counts.subdirectories += 1;
                } else {
                    counts.files += 1;
                }
                counts
            });
        Ok(Some(counts))
    }

    /// Re-read every expanded directory, keeping expansion state and the
    /// cursor path where they still exist.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the root cannot be read; a deeper
    /// directory that vanished is collapsed instead.
    pub fn refresh(&mut self, workspace: &mut Workspace) -> Result<(), WorkspaceError> {
        let cursor_path = self.current().map(|row| row.path.clone());
        self.sync_review_paths(workspace);
        self.admitted = Admitted::new(
            self.shown,
            &self.status,
            &self.admitted_review_paths,
            self.snapshot_paths.as_deref(),
        );
        // From the nodes, not the rows: a directory the rules hide keeps
        // its expansion for when they list it again (ADR 0068).
        let mut expanded = Vec::new();
        expanded_dirs(&self.root, &PathBuf::new(), &mut expanded);
        self.root.children = Some(read_children(
            workspace,
            Path::new(""),
            self.shown.filter(),
            &self.deleted,
        )?);
        for path in expanded {
            if let Err(error) = self.expand_path(workspace, &path) {
                tracing::debug!(%error, "directory gone during refresh");
            }
        }
        self.rebuild();
        if let Some(path) = cursor_path {
            self.select_path(&path);
        }
        Ok(())
    }

    /// Re-read the listing that holds `dir` after a file appeared,
    /// vanished, or was renamed in it (ADR 0028).
    ///
    /// A directory the tree has read is re-read whether it is expanded or
    /// collapsed, so a collapsed listing cannot go stale behind the
    /// reader's back. One the tree has never read is left alone: it is
    /// read when it is expanded. When `dir` itself is not in the tree —
    /// the agent made a directory and wrote into it in one burst — the
    /// nearest listing above it is re-read instead, which is what brings
    /// the new directory into view. Expansion state of surviving
    /// subdirectories and the cursor path are kept. Returns whether
    /// anything was re-read.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the directory cannot be read; a
    /// directory that vanished is collapsed and reported as unchanged.
    pub fn refresh_dir(
        &mut self,
        workspace: &mut Workspace,
        dir: &Path,
    ) -> Result<bool, WorkspaceError> {
        let Some(dir) = listing_of(&self.root, dir) else {
            return Ok(false);
        };
        let dir = dir.as_path();
        let filter = self.shown.filter();
        let Some(node) = find_node(&mut self.root, dir) else {
            return Ok(false);
        };
        let fresh = match read_children(workspace, dir, filter, &self.deleted) {
            Ok(fresh) => fresh,
            Err(error) if !workspace.root().join(dir).is_dir() => {
                tracing::debug!(%error, "directory gone; collapsing it");
                node.expanded = false;
                node.children = None;
                self.rebuild();
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let mut old = node.children.take().unwrap_or_default();
        node.children = Some(
            fresh
                .into_iter()
                .map(|child| {
                    match old.iter().position(|o| {
                        o.name == child.name && o.is_dir == child.is_dir && o.kind == child.kind
                    }) {
                        Some(index) => {
                            let mut kept = old.swap_remove(index);
                            if child.deleted && !kept.deleted {
                                kept.mark_deleted();
                            }
                            kept.deleted = child.deleted;
                            kept
                        }
                        None => child,
                    }
                })
                .collect(),
        );
        let cursor_path = self.current().map(|row| row.path.clone());
        self.rebuild();
        if let Some(path) = cursor_path {
            self.select_path(&path);
        }
        Ok(true)
    }

    /// Whether `path` is one of the visible rows.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.rows.iter().any(|row| row.path == path)
    }

    /// Move the cursor down `n` rows.
    pub fn move_down(&mut self, n: usize) {
        let last = self.rows.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add(n).min(last);
    }

    /// Move the cursor up `n` rows.
    pub fn move_up(&mut self, n: usize) {
        self.cursor = self.cursor.saturating_sub(n);
    }

    /// Move the cursor to the first row.
    pub fn goto_top(&mut self) {
        self.cursor = 0;
    }

    /// Move the cursor to the last row.
    pub fn goto_bottom(&mut self) {
        self.cursor = self.rows.len().saturating_sub(1);
    }

    /// Put the cursor on row `index` (a click); out-of-range is clamped.
    pub fn set_cursor(&mut self, index: usize) {
        self.cursor = index.min(self.rows.len().saturating_sub(1));
    }

    /// Open a file or toggle a directory without moving its cursor.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when a directory cannot be read.
    pub fn activate(
        &mut self,
        workspace: &mut Workspace,
    ) -> Result<Option<Activation>, WorkspaceError> {
        let Some(row) = self.current().cloned() else {
            return Ok(None);
        };
        if !row.is_dir {
            return Ok(Some(Activation::Open(row.path)));
        }
        if row.expanded {
            self.collapse_path(&row.path);
        } else {
            self.expand_path(workspace, &row.path)?;
        }
        self.rebuild();
        Ok(Some(Activation::Toggled))
    }

    /// `l` / Right: expand a directory or descend into an expanded one;
    /// on a file this leaves the cursor in place without opening it.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when a directory cannot be read.
    pub fn expand(
        &mut self,
        workspace: &mut Workspace,
    ) -> Result<Option<Activation>, WorkspaceError> {
        let Some(row) = self.current().filter(|row| row.is_dir).cloned() else {
            return Ok(None);
        };
        if row.expanded {
            if self
                .rows
                .get(self.cursor + 1)
                .is_some_and(|next| next.depth > row.depth)
            {
                self.cursor += 1;
            }
            return Ok(None);
        }
        self.activate(workspace)
    }

    /// `h` / Left: collapse the directory under the cursor, or move to the
    /// parent of a file or collapsed directory.
    pub fn collapse(&mut self) {
        let Some(row) = self.current().cloned() else {
            return;
        };
        if row.is_dir && row.expanded {
            self.collapse_path(&row.path);
            self.rebuild();
            self.select_path(&row.path);
        } else if let Some(parent) = row.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            let parent = parent.to_path_buf();
            self.select_path(&parent);
        }
    }

    /// Expand all listed directories, or collapse them when all are expanded.
    ///
    /// Filters still apply, and directory symlinks are not expanded recursively.
    /// The cursor keeps its path or the nearest visible ancestor when collapsing hides it.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when a directory cannot be read. Directories
    /// expanded before that failure remain expanded and visible.
    pub fn toggle_all(&mut self, workspace: &mut Workspace) -> Result<(), WorkspaceError> {
        let cursor = self.current().map(|row| row.path.clone());
        let unfold = self.rows.iter().any(|row| {
            row.is_dir
                && !row.expanded
                && find_node(&mut self.root, &row.path)
                    .is_some_and(|node| node.kind == EntryKind::Dir)
        });
        let result = if unfold {
            self.expand_all(workspace)
        } else {
            let mut expanded = Vec::new();
            expanded_dirs(&self.root, Path::new(""), &mut expanded);
            for path in expanded {
                if self.admitted.admits(&path, true) {
                    self.collapse_path(&path);
                }
            }
            Ok(())
        };
        self.rebuild();
        if let Some(cursor) = cursor {
            for path in cursor.ancestors() {
                if self.select_path(path) {
                    break;
                }
            }
        }
        result
    }

    fn expand_all(&mut self, workspace: &mut Workspace) -> Result<(), WorkspaceError> {
        let mut pending = vec![PathBuf::new()];
        while let Some(path) = pending.pop() {
            self.expand_path(workspace, &path)?;
            if let Some(children) =
                find_node(&mut self.root, &path).and_then(|node| node.children.as_ref())
            {
                pending.extend(
                    children
                        .iter()
                        .filter(|child| child.kind == EntryKind::Dir)
                        .filter_map(|child| {
                            let path = path.join(&child.name);
                            self.admitted.admits(&path, true).then_some(path)
                        }),
                );
            }
        }
        Ok(())
    }

    /// Expand every directory on the way to `path` and put the cursor on it.
    /// Returns `false` if the path is not in the tree (hidden or missing).
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when a directory on the way cannot be read.
    pub fn reveal(
        &mut self,
        workspace: &mut Workspace,
        path: &Path,
    ) -> Result<bool, WorkspaceError> {
        Ok(self.expand_to(workspace, path)? && self.select_path(path))
    }

    /// Expand every directory on the way to `path` without moving the
    /// cursor (ADR 0028: a followed file is shown, not selected). Returns
    /// `false` if the path is not in the tree (hidden or missing).
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when a directory on the way cannot be read.
    pub fn expand_to(
        &mut self,
        workspace: &mut Workspace,
        path: &Path,
    ) -> Result<bool, WorkspaceError> {
        let cursor_path = self.current().map(|row| row.path.clone());
        let mut prefix = PathBuf::new();
        let components: Vec<_> = path.components().collect();
        let mut found = true;
        for component in components.iter().take(components.len().saturating_sub(1)) {
            let Component::Normal(name) = component else {
                found = false;
                break;
            };
            prefix.push(name);
            if !self.expand_path(workspace, &prefix)? {
                found = false;
                break;
            }
        }
        self.rebuild();
        if let Some(cursor_path) = cursor_path {
            self.select_path(&cursor_path);
        }
        Ok(found && self.contains(path))
    }

    fn select_path(&mut self, path: &Path) -> bool {
        if let Some(index) = self.rows.iter().position(|row| row.path == path) {
            self.cursor = index;
            true
        } else {
            self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
            false
        }
    }

    /// Expand the directory at `path`, reading it if needed. Returns whether
    /// it exists in the tree.
    fn expand_path(
        &mut self,
        workspace: &mut Workspace,
        path: &Path,
    ) -> Result<bool, WorkspaceError> {
        let filter = self.shown.filter();
        let Some(node) = find_node(&mut self.root, path) else {
            return Ok(false);
        };
        if !node.is_dir {
            return Ok(false);
        }
        if node.children.is_none() {
            node.children = Some(read_children(workspace, path, filter, &self.deleted)?);
        }
        node.expanded = true;
        Ok(true)
    }

    fn collapse_path(&mut self, path: &Path) {
        if let Some(node) = find_node(&mut self.root, path) {
            node.expanded = false;
        }
    }

    fn rebuild(&mut self) {
        self.deleted.sync(&mut self.root, Path::new(""));
        let mut rows = Vec::new();
        push_rows(&self.root, &PathBuf::new(), 0, &self.admitted, &mut rows);
        self.rows = rows;
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }
}

/// Every expanded directory under `node`, root-relative, `node` itself
/// aside.
fn expanded_dirs(node: &Node, path: &Path, out: &mut Vec<PathBuf>) {
    let Some(children) = node.children.as_ref() else {
        return;
    };
    for child in children.iter().filter(|child| child.is_dir) {
        let child_path = path.join(&child.name);
        if child.expanded {
            out.push(child_path.clone());
        }
        expanded_dirs(child, &child_path, out);
    }
}

fn read_children(
    workspace: &mut Workspace,
    path: &Path,
    filter: Filter,
    deleted: &Deleted,
) -> Result<Vec<Node>, WorkspaceError> {
    let entries = match workspace.list_dir_with(path, filter) {
        Ok(entries) => entries,
        Err(_)
            if deleted.children.contains_key(path)
                && matches!(workspace.root().join(path).try_exists(), Ok(false)) =>
        {
            // A removed directory is still browsable through its deleted files.
            Vec::new()
        }
        Err(error) => return Err(error),
    };
    let mut children = entries
        .into_iter()
        .map(|entry| Node {
            is_dir: entry.is_dir(),
            kind: if entry.is_dir() && !entry.is_symlink() {
                EntryKind::Dir
            } else {
                EntryKind::File
            },
            name: entry.name().to_owned(),
            expanded: false,
            children: None,
            deleted: false,
        })
        .collect();
    deleted.merge(path, &mut children);
    Ok(children)
}

/// The listing to re-read so the tree reflects a change at `dir`: `dir`
/// itself when the tree has read it, or the deepest read directory above
/// it when `dir` is not in the tree. `None` when the tree has read no
/// listing that would show `dir`, which is one it has never expanded.
fn listing_of(root: &Node, dir: &Path) -> Option<PathBuf> {
    let mut node = root;
    let mut prefix = PathBuf::new();
    for component in dir.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        let name = name.to_str()?;
        // An unread directory on the way holds no listing to update.
        let children = node.children.as_ref()?;
        let Some(child) = children.iter().find(|c| c.name == name && c.is_dir) else {
            // `dir` is not in the tree, but the listing that would name
            // it is the one this change belongs to.
            return Some(prefix);
        };
        node = child;
        prefix.push(name);
    }
    node.children.is_some().then_some(prefix)
}

fn find_node<'a>(root: &'a mut Node, path: &Path) -> Option<&'a mut Node> {
    let mut node = root;
    for component in path.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        let name = name.to_str()?;
        node = node
            .children
            .as_mut()?
            .iter_mut()
            .find(|child| child.name == name)?;
    }
    Some(node)
}

/// The rows under an expanded `node`, in listing order, leaving out what
/// the rules do not admit and everything beneath it (ADR 0068).
fn push_rows(node: &Node, path: &Path, depth: usize, admitted: &Admitted, rows: &mut Vec<Row>) {
    let Some(children) = node.children.as_ref().filter(|_| node.expanded) else {
        return;
    };
    for child in children {
        let child_path = path.join(&child.name);
        if !admitted.admits(&child_path, child.is_dir) {
            continue;
        }
        rows.push(Row {
            path: child_path.clone(),
            name: child.name.clone(),
            depth,
            is_dir: child.is_dir,
            expanded: child.expanded,
        });
        push_rows(child, &child_path, depth + 1, admitted, rows);
    }
}
