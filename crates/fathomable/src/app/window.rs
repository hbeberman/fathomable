// @okf-doc: /decisions/0056-the-leader-trimmed.md
//! Moving the keys between the panes: Helix's window submenu, `Space w
//! h/j/k/l/w`, and `Space Space`, over the panes that are shown and the
//! review list while it is open (ADR 0056).
//!
//! A move with nowhere to go does nothing. The text column is the
//! review list while it is open and the view otherwise; the files pane
//! is to its left, the threads pane below the files pane.

use super::{App, Focus};

impl App {
    /// `Space w h`: the pane left of the text. The files pane, or the
    /// threads pane when the files pane is hidden; the files pane is
    /// shown when neither is.
    pub(crate) fn window_left(&mut self) {
        if !matches!(self.focus, Focus::View | Focus::Review) {
            return;
        }
        if !self.rail.tree && self.rail.threads {
            self.focus_threads_pane();
        } else {
            self.focus_tree();
        }
    }

    /// `Space w l`: back to the text.
    pub(crate) fn window_right(&mut self) {
        if matches!(self.focus, Focus::Tree | Focus::ThreadsPane) {
            self.focus = self.column_focus();
        }
    }

    /// `Space w j`: from the files pane to the threads pane when shown.
    pub(crate) fn window_down(&mut self) {
        if self.focus == Focus::Tree && self.rail.threads {
            self.focus_threads_pane();
        }
    }

    /// `Space w k`: from the threads pane to the files pane when shown.
    pub(crate) fn window_up(&mut self) {
        if self.focus == Focus::ThreadsPane && self.rail.tree {
            self.focus_tree();
        }
    }

    /// `Space w w`, `Space Space`: text, files pane, threads pane, text,
    /// skipping hidden panes.
    pub(crate) fn window_next(&mut self) {
        let in_text = matches!(self.focus, Focus::View | Focus::Review);
        if in_text && self.rail.tree {
            self.focus_tree();
        } else if self.rail.threads && (in_text || self.focus == Focus::Tree) {
            self.focus_threads_pane();
        } else if !in_text {
            self.focus = self.column_focus();
        }
    }

    /// The text column's focus: the review list while it is open.
    fn column_focus(&self) -> Focus {
        if self.review_list.is_open() {
            Focus::Review
        } else {
            Focus::View
        }
    }

    /// Show the files pane with its highlight on the current file and
    /// give it the keys.
    fn focus_tree(&mut self) {
        if !self.rail.tree {
            if !self.ensure_tree() {
                return;
            }
            self.rail.tree = true;
        }
        self.reveal_current();
        self.focus = Focus::Tree;
        self.relayout();
    }
}
