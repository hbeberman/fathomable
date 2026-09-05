// @okf-doc: /decisions/0023-sidebar-paging.md
//! The files pane's hands on the app: tree operations, and the rule that
//! the highlighted file is the one the main pane shows (ADR 0023; the
//! pane is the sidebar's upper half since ADR 0049, the column named by
//! ADR 0057).

use fathomable_core::tree::{Activation, Tree};
use fathomable_core::workspace::Workspace;

use super::view::Effect;
use super::{App, Focus, TREE_SCROLLOFF};

impl App {
    /// Run `f` on the tree, then keep the cursor on screen.
    pub(crate) fn with_tree(
        &mut self,
        f: impl FnOnce(&mut Tree, &mut Workspace) -> Option<Activation>,
    ) {
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let activation = f(tree, &mut self.workspace);
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

    /// A click on files pane row `row` (screen coordinates): activate the
    /// row but stay in the tree. A click pages the viewer just as the
    /// wheel does (ADR 0023); only `Enter` commits focus to the view.
    pub(crate) fn tree_click(&mut self, row: usize) {
        let index = self.tree_scroll + row;
        self.with_tree_result(|tree, workspace| {
            if index >= tree.rows().len() {
                return Ok(None);
            }
            tree.set_cursor(index);
            tree.activate(workspace)
        });
        self.focus = Focus::Tree;
    }

    /// A right-click on tree row `row` (ADR 0050): the highlight moves
    /// there and the main pane shows the file as the wheel does; a
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
        self.focus = Focus::Tree;
        if self.tree().map(Tree::cursor) != before {
            self.show_highlight();
        }
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

    /// Show the file under the tree cursor, leaving focus where it is: the
    /// tree highlight is what the main pane shows (ADR 0023), so `j`,
    /// `k`, and the wheel page the viewer through files. A directory row
    /// leaves the pane on the file it already shows.
    pub(super) fn show_highlight(&mut self) {
        let Some(path) = self
            .tree()
            .and_then(Tree::current)
            .filter(|row| !row.is_dir())
            .map(|row| row.path().to_path_buf())
        else {
            return;
        };
        if self.current_path() == path {
            return;
        }
        let focus = self.focus;
        self.open(&path);
        self.focus = focus;
    }
}
