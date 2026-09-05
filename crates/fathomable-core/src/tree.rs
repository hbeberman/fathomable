// @okf-doc: /decisions/0012-workspace-mode.md
//! The tree pane's tree: lazily expanded directories with a cursor.
//!
//! [`Tree`] is plain data over a [`Workspace`]: directories are read when
//! first expanded, and the visible rows are recomputed after every change so
//! the terminal can draw them and the keys can move a cursor over them
//! (ADR 0012). Filesystem access always goes through the workspace so ignore
//! rules apply.

use std::path::{Component, Path, PathBuf};

use crate::workspace::{Filter, Workspace, WorkspaceError};

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

#[derive(Debug, Clone)]
struct Node {
    name: String,
    is_dir: bool,
    expanded: bool,
    /// `None` until first expanded.
    children: Option<Vec<Node>>,
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
    filter: Filter,
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
                expanded: true,
                children: None,
            },
            rows: Vec::new(),
            cursor: 0,
            filter: Filter::Visible,
        };
        tree.refresh(workspace)?;
        Ok(tree)
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

    /// Which entries are listed.
    #[must_use]
    pub fn filter(&self) -> Filter {
        self.filter
    }

    /// Change the filter and re-read expanded directories.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when the root cannot be read.
    pub fn set_filter(
        &mut self,
        workspace: &mut Workspace,
        filter: Filter,
    ) -> Result<(), WorkspaceError> {
        self.filter = filter;
        self.refresh(workspace)
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
        let expanded: Vec<PathBuf> = self
            .rows
            .iter()
            .filter(|row| row.is_dir && row.expanded)
            .map(|row| row.path.clone())
            .collect();
        self.root.children = Some(read_children(workspace, Path::new(""), self.filter)?);
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
        let filter = self.filter;
        let Some(dir) = listing_of(&self.root, dir) else {
            return Ok(false);
        };
        let dir = dir.as_path();
        let Some(node) = find_node(&mut self.root, dir) else {
            return Ok(false);
        };
        let fresh = match read_children(workspace, dir, filter) {
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
                    match old
                        .iter()
                        .position(|o| o.name == child.name && o.is_dir == child.is_dir)
                    {
                        Some(index) => old.swap_remove(index),
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

    /// Enter, `l`, or a click: open a file or toggle a directory.
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
    /// on a file this is the same as [`Tree::activate`].
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceError`] when a directory cannot be read.
    pub fn expand(
        &mut self,
        workspace: &mut Workspace,
    ) -> Result<Option<Activation>, WorkspaceError> {
        let Some(row) = self.current().cloned() else {
            return Ok(None);
        };
        if row.is_dir && row.expanded {
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
        let filter = self.filter;
        let Some(node) = find_node(&mut self.root, path) else {
            return Ok(false);
        };
        if !node.is_dir {
            return Ok(false);
        }
        if node.children.is_none() {
            node.children = Some(read_children(workspace, path, filter)?);
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
        let mut rows = Vec::new();
        push_rows(&self.root, &PathBuf::new(), 0, &mut rows);
        self.rows = rows;
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }
}

fn read_children(
    workspace: &mut Workspace,
    path: &Path,
    filter: Filter,
) -> Result<Vec<Node>, WorkspaceError> {
    Ok(workspace
        .list_dir_with(path, filter)?
        .into_iter()
        .map(|entry| Node {
            is_dir: entry.is_dir(),
            name: entry.name().to_owned(),
            expanded: false,
            children: None,
        })
        .collect())
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

fn push_rows(node: &Node, path: &Path, depth: usize, rows: &mut Vec<Row>) {
    let Some(children) = node.children.as_ref().filter(|_| node.expanded) else {
        return;
    };
    for child in children {
        let child_path = path.join(&child.name);
        rows.push(Row {
            path: child_path.clone(),
            name: child.name.clone(),
            depth,
            is_dir: child.is_dir,
            expanded: child.expanded,
        });
        push_rows(child, &child_path, depth + 1, rows);
    }
}
