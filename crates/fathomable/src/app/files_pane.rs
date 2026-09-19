// @okf-doc: /decisions/0023-sidebar-paging.md
//! The files pane's hands on the app: tree operations, and the rule that
//! the highlighted file or directory summary is what the main pane shows
//! (ADR 0023; the pane is the sidebar's upper half since ADR 0049, the
//! column named by ADR 0057).

use std::path::PathBuf;

use fathomable_core::tree::{Activation, DirectoryCounts, Tree};
use fathomable_core::workspace::Workspace;

use super::view::Effect;
use super::{App, Focus, TREE_SCROLLOFF};

#[derive(Debug)]
pub(super) struct TreeRequest {
    root: PathBuf,
    tree: Tree,
    limits: fathomable_core::config::LimitsConfig,
}

#[derive(Debug)]
pub(super) struct TreeResult {
    pub(super) tree: Option<Tree>,
    pub(super) incomplete: Option<String>,
}

pub(super) fn expand_tree(
    request: TreeRequest,
    cancellation: fathomable_core::workspace::Cancellation,
) -> TreeResult {
    let mut workspace = match Workspace::discover(request.root) {
        Ok(workspace) => workspace,
        Err(error) => {
            return TreeResult {
                tree: None,
                incomplete: Some(error.to_string()),
            };
        }
    };
    workspace.set_limits(request.limits);
    workspace.set_cancellation(cancellation);
    let mut tree = request.tree;
    let incomplete = tree
        .toggle_all(&mut workspace)
        .err()
        .map(|error| error.to_string())
        .or_else(|| tree.listing_incomplete().map(ToString::to_string));
    TreeResult {
        tree: Some(tree),
        incomplete,
    }
}

/// The selected directory and the direct counts read for its card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DirectorySelection {
    path: PathBuf,
    counts: Option<DirectoryCounts>,
}

/// The directory facts drawn beneath the File surface's chrome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirectoryInfo {
    pub(crate) path: PathBuf,
    pub(crate) files: Option<usize>,
    pub(crate) subdirectories: Option<usize>,
    pub(crate) changed_files: usize,
    pub(crate) added: usize,
    pub(crate) removed: usize,
    pub(crate) active_threads: usize,
    pub(crate) proposed_threads: usize,
    pub(crate) resolved_threads: usize,
}

impl App {
    pub(super) fn cancel_tree_scan(&mut self) {
        if self.tree_walk.pending() {
            self.tree_issue =
                Some("directory discovery cancelled; coverage is incomplete".to_owned());
        }
        self.tree_walk.cancel();
    }

    pub(crate) fn toggle_all_directories(&mut self) {
        let Some(tree) = self.tree.clone() else {
            return;
        };
        self.tree_issue = Some("scanning directories; coverage is incomplete".to_owned());
        if let Err(error) = self.tree_walk.submit(TreeRequest {
            root: self.workspace.root().to_path_buf(),
            tree,
            limits: self.workspace.limits().clone(),
        }) {
            let message = format!("cannot start directory discovery: {error}");
            self.tree_issue = Some(message.clone());
            self.notice(message);
        }
    }

    /// Run `f` on the tree, then keep the cursor on screen.
    pub(crate) fn with_tree(
        &mut self,
        f: impl FnOnce(&mut Tree, &mut Workspace) -> Option<Activation>,
    ) {
        self.cancel_tree_scan();
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let activation = f(tree, &mut self.workspace);
        if let Some(error) = self.workspace.listing_incomplete() {
            self.tree_issue = Some(error.to_string());
        }
        self.scroll_tree();
        if let Some(Activation::Open(path)) = activation {
            self.open(&path);
        }
    }

    /// A tree operation that can fail: report the error on the status line.
    pub(crate) fn with_tree_result(
        &mut self,
        f: impl FnOnce(
            &mut Tree,
            &mut Workspace,
        )
            -> Result<Option<Activation>, fathomable_core::workspace::WorkspaceError>,
    ) {
        let mut failure = None;
        self.with_tree(|tree, workspace| match f(tree, workspace) {
            Ok(activation) => activation,
            Err(error) => {
                failure = Some(error.to_string());
                None
            }
        });
        if let Some(message) = failure {
            self.notice(message);
        }
    }

    /// Focus File list and preview its row without changing the main surface.
    pub(crate) fn tree_click(&mut self, row: usize) {
        let index = self.tree_scroll + row;
        if self.tree().is_none_or(|tree| index >= tree.rows().len()) {
            self.focus_pane(Focus::Tree);
            return;
        }
        self.cancel_tree_target();
        self.with_tree_result(|tree, workspace| {
            tree.set_cursor(index);
            if tree.current().is_some_and(|entry| !entry.is_dir()) {
                return Ok(None);
            }
            tree.activate(workspace)
        });
        self.focus_pane(Focus::Tree);
        self.show_highlight();
    }

    /// A right-click on tree row `row` (ADR 0050): the highlight moves
    /// there and the main pane previews the file; a
    /// directory is neither expanded nor collapsed.
    pub(crate) fn tree_point(&mut self, row: usize) {
        let index = self.tree_scroll + row;
        let before = self.tree().map(Tree::cursor);
        self.with_tree(|tree, _| {
            if index < tree.rows().len() {
                tree.set_cursor(index);
            }
            None
        });
        if self.tree().map(Tree::cursor) != before {
            self.cancel_tree_target();
            self.show_highlight();
        }
        self.focus = Focus::Tree;
    }

    /// `y` in the tree (ADR 0050): copy the highlighted row's path,
    /// relative to the workspace root as the tree shows it.
    pub(crate) fn copy_tree_path(&mut self) -> Effect {
        let Some(path) = self
            .tree()
            .and_then(Tree::current)
            .map(|row| row.path().to_string_lossy().into_owned())
        else {
            return Effect::None;
        };
        self.notice(format!("copied {path}"));
        Effect::Copy(path)
    }

    /// `Y` in the tree: copy the highlighted row's absolute path.
    pub(crate) fn copy_tree_full_path(&mut self) -> Effect {
        let Some(path) = self
            .tree()
            .and_then(Tree::current)
            .map(|row| self.workspace.root().join(row.path()))
        else {
            return Effect::None;
        };
        let path = path.to_string_lossy().into_owned();
        self.notice(format!("copied {path}"));
        Effect::Copy(path)
    }

    pub(super) fn scroll_tree(&mut self) {
        // The threads pane (ADR 0027) takes rows from the tree.
        let rows = self.tree_rows().saturating_sub(1).max(1);
        let Some(tree) = self.tree.as_ref() else {
            return;
        };
        let cursor = tree.cursor();
        let off = TREE_SCROLLOFF.min(rows.saturating_sub(1) / 2);
        if cursor < self.tree_scroll + off {
            self.tree_scroll = cursor.saturating_sub(off);
        }
        if cursor + off >= self.tree_scroll + rows {
            self.tree_scroll = (cursor + off + 1).saturating_sub(rows);
        }
        let max = tree.rows().len().saturating_sub(rows);
        self.tree_scroll = self.tree_scroll.min(max);
    }

    /// Move only the File-list viewport, leaving its cursor and preview.
    pub(crate) fn scroll_tree_by(&mut self, delta: isize) {
        let rows = self.tree_rows().saturating_sub(1).max(1);
        let max = self
            .tree
            .as_ref()
            .map_or(0, |tree| tree.rows().len().saturating_sub(rows));
        self.tree_scroll = self.tree_scroll.saturating_add_signed(delta).min(max);
    }

    /// Remember and reveal a workspace-navigation destination in Files.
    pub(super) fn synchronize_tree_to(&mut self, path: &std::path::Path) {
        self.tree_target = Some(path.to_path_buf());
        self.reveal_tree_path(path);
    }

    /// Let explicit Files navigation supersede a remembered destination.
    pub(super) fn cancel_tree_target(&mut self) {
        self.tree_target = None;
    }

    /// Retry a remembered destination after the Files listing changes.
    pub(super) fn refresh_tree_target(&mut self) {
        if let Some(path) = self.tree_target.clone() {
            self.reveal_tree_path(&path);
        } else {
            self.scroll_tree();
        }
    }

    fn reveal_tree_path(&mut self, path: &std::path::Path) {
        if path.as_os_str().is_empty() {
            return;
        }
        let revealed = match self.tree.as_mut() {
            Some(tree) => match tree.reveal(&mut self.workspace, path) {
                Ok(revealed) => revealed,
                Err(error) => {
                    tracing::debug!(%error, "cannot reveal file in tree");
                    false
                }
            },
            None => false,
        };
        if !self.sidebar.tree {
            return;
        }
        if revealed {
            self.center_tree();
        } else {
            self.scroll_tree();
        }
    }

    fn center_tree(&mut self) {
        let rows = self.tree_rows().saturating_sub(1).max(1);
        let Some(tree) = self.tree.as_ref() else {
            return;
        };
        let max = tree.rows().len().saturating_sub(rows);
        self.tree_scroll = tree.cursor().saturating_sub(rows / 2).min(max);
    }

    /// Show the file or directory summary under the tree cursor, leaving
    /// focus where it is (ADR 0023), so `j` and `k` page the
    /// main pane through entries.
    pub(super) fn show_highlight(&mut self) {
        let Some((path, is_dir)) = self
            .tree()
            .and_then(Tree::current)
            .map(|row| (row.path().to_path_buf(), row.is_dir()))
        else {
            self.directory = None;
            return;
        };
        if is_dir {
            let counts = match self.tree.as_mut().and_then(|tree| {
                match tree.current_directory_counts(&mut self.workspace) {
                    Ok(counts) => counts.map(Ok),
                    Err(error) => Some(Err(error)),
                }
            }) {
                Some(Ok(counts)) => Some(counts),
                Some(Err(error)) => {
                    self.notice(error.to_string());
                    None
                }
                None => None,
            };
            self.directory = Some(DirectorySelection { path, counts });
            self.relayout();
            return;
        }
        let had_directory = self.directory.take().is_some();
        if self.current_path() == path {
            if had_directory {
                self.relayout();
            }
            return;
        }
        let focus = self.focus;
        self.preview_file(&path);
        self.focus = focus;
    }

    /// Refresh the selected directory's direct counts after its listing or
    /// the files-pane filters change.
    pub(super) fn refresh_directory_selection(&mut self) {
        let Some(path) = self
            .directory
            .as_ref()
            .map(|directory| directory.path.clone())
        else {
            return;
        };
        let selected = self
            .tree()
            .and_then(Tree::current)
            .filter(|row| row.is_dir() && row.path() == path);
        if selected.is_none() {
            self.directory = None;
            self.relayout();
            return;
        }
        self.show_highlight();
    }

    /// Facts about the directory currently replacing the file body.
    pub(crate) fn directory_info(&self) -> Option<DirectoryInfo> {
        let directory = self.directory.as_ref()?;
        let shown = self
            .tree
            .as_ref()
            .map_or_else(Default::default, Tree::shown);
        let comparison_status = self.comparison_status();
        let changes: Vec<_> = comparison_status
            .entries()
            .iter()
            .filter(|entry| entry.path().starts_with(&directory.path))
            .filter(|entry| shown.untracked() || !entry.changes().is_only_untracked())
            .collect();
        let changed_files = changes.len();
        let added = changes.iter().map(|entry| entry.added()).sum();
        let removed = changes.iter().map(|entry| entry.removed()).sum();
        let threads = self.directory_thread_counts(&directory.path);
        Some(DirectoryInfo {
            path: directory.path.clone(),
            files: directory.counts.map(DirectoryCounts::files),
            subdirectories: directory.counts.map(DirectoryCounts::subdirectories),
            changed_files,
            added,
            removed,
            active_threads: threads.active,
            proposed_threads: threads.proposed,
            resolved_threads: threads.resolved,
        })
    }

    /// The selected directory replacing the file body.
    pub(crate) fn directory_path(&self) -> Option<&std::path::Path> {
        self.directory
            .as_ref()
            .filter(|_| !self.review_list().is_open())
            .map(|directory| directory.path.as_path())
    }
}
