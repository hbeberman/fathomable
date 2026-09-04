// @okf-doc: /decisions/0023-sidebar-paging.md
//! The sidebar's hands on the app: tree operations, and the rule that the
//! highlighted file is the one the main pane shows (ADR 0023).

use fathomable_core::tree::{Activation, Tree};
use fathomable_core::workspace::{Filter, Workspace};

use super::{App, Focus, SIDEBAR_SCROLLOFF};

impl App {
    /// Run `f` on the tree, then keep the cursor on screen.
    pub fn with_tree(&mut self, f: impl FnOnce(&mut Tree, &mut Workspace) -> Option<Activation>) {
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let activation = f(tree, &mut self.workspace);
        self.scroll_sidebar();
        if let Some(Activation::Open(path)) = activation {
            self.open(&path);
        }
    }

    /// A tree operation that can fail: report the error on the status line.
    pub fn with_tree_result(
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

    /// `R` in the tree: re-read directories and drop the picker indexes.
    pub fn refresh_tree(&mut self) {
        self.file_index = None;
        self.all_index = None;
        self.with_tree_result(|tree, workspace| tree.refresh(workspace).map(|()| None));
        self.notice("tree refreshed");
    }

    /// `Space r .`: show the tree when it is hidden and put its highlight
    /// on the current file, keeping focus where it is (ADR 0049).
    pub fn reveal_in_tree(&mut self) {
        if !self.sidebar_visible {
            if !self.ensure_tree() {
                return;
            }
            self.sidebar_visible = true;
            self.relayout();
        }
        self.reveal_current();
    }

    /// `I` in the tree: toggle ignored entries.
    pub fn toggle_ignored(&mut self) {
        let filter = match self.tree.as_ref().map(Tree::filter) {
            Some(Filter::Visible) => Filter::All,
            _ => Filter::Visible,
        };
        self.with_tree_result(move |tree, workspace| {
            tree.set_filter(workspace, filter).map(|()| None)
        });
    }

    /// A click on sidebar row `row` (screen coordinates): activate the
    /// row but stay in the tree. A click pages the viewer just as the
    /// wheel does (ADR 0023); only `Enter` commits focus to the view.
    pub fn sidebar_click(&mut self, row: usize) {
        let index = self.sidebar_scroll + row;
        self.with_tree_result(|tree, workspace| {
            if index >= tree.rows().len() {
                return Ok(None);
            }
            tree.set_cursor(index);
            tree.activate(workspace)
        });
        self.focus = Focus::Sidebar;
    }

    pub(super) fn scroll_sidebar(&mut self) {
        // The file-threads pane (ADR 0027) takes rows from the tree.
        let rows = self.sidebar_rows().saturating_sub(1).max(1);
        let Some(tree) = self.tree.as_ref() else {
            return;
        };
        let cursor = tree.cursor();
        let off = SIDEBAR_SCROLLOFF.min(rows.saturating_sub(1) / 2);
        if cursor < self.sidebar_scroll + off {
            self.sidebar_scroll = cursor.saturating_sub(off);
        }
        if cursor + off >= self.sidebar_scroll + rows {
            self.sidebar_scroll = (cursor + off + 1).saturating_sub(rows);
        }
        let max = tree.rows().len().saturating_sub(rows);
        self.sidebar_scroll = self.sidebar_scroll.min(max);
    }

    /// Show the file under the tree cursor, leaving focus where it is: the
    /// sidebar highlight is what the main pane shows (ADR 0023), so `j`,
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
