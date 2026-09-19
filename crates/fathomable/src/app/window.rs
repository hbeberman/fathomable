// @okf-doc: /decisions/0091-pane-focus-navigation.md
//! Focus traversal among the displayed main view and the sidebar lists.
//!
//! Bare `w` and `W` move forward and backward; `F` and `T` name the
//! sidebar lists and show a hidden one before focusing it.

use super::sidebar::SIDEBAR_MIN_WIDTH;
use super::view::Mode;
use super::{App, Focus};

/// Main content keeps a header, one useful row, and its footer.
const MAIN_MIN_ROWS: usize = 3;
/// A file list keeps its header and one useful row.
const FILE_LIST_MIN_ROWS: usize = 2;
/// A thread list keeps its divider, header, one useful row, and footer.
const THREAD_LIST_MIN_ROWS: usize = 4;

impl App {
    /// Return from a sidebar list to the displayed main view.
    #[cfg(test)]
    pub(crate) fn window_right(&mut self) {
        if matches!(self.focus, Focus::Tree | Focus::ThreadsPane) {
            self.focus = self.displayed_main_focus();
        }
    }

    /// `w`: main, File list, Thread list, main, skipping hidden lists.
    pub(crate) fn window_next(&mut self) {
        match self.focus {
            Focus::View | Focus::Review if self.sidebar.tree => self.focus_tree(),
            Focus::View | Focus::Review | Focus::Tree if self.sidebar.threads => {
                self.focus_threads_pane();
            }
            Focus::Tree | Focus::ThreadsPane => self.focus = self.displayed_main_focus(),
            Focus::View | Focus::Review => {}
        }
    }

    /// `W`: main, Thread list, File list, main, skipping hidden lists.
    pub(crate) fn window_previous(&mut self) {
        match self.focus {
            Focus::View | Focus::Review if self.sidebar.threads => self.focus_threads_pane(),
            Focus::View | Focus::Review | Focus::ThreadsPane if self.sidebar.tree => {
                self.focus_tree();
            }
            Focus::Tree | Focus::ThreadsPane => self.focus = self.displayed_main_focus(),
            Focus::View | Focus::Review => {}
        }
    }

    /// `F`: show and focus the File list.
    pub(crate) fn window_files(&mut self) {
        self.focus_tree();
    }

    /// `T`: show and focus the Thread list.
    pub(crate) fn window_threads(&mut self) {
        self.focus_threads_pane();
    }

    /// Focus belonging to the main view that is currently displayed.
    pub(super) fn displayed_main_focus(&self) -> Focus {
        if self.review_list.is_open() {
            Focus::Review
        } else {
            Focus::View
        }
    }

    /// Show the File list without changing its cursor or preview.
    fn focus_tree(&mut self) {
        let mut shown = false;
        if !self.sidebar.tree {
            if !self.ensure_tree() {
                return;
            }
            self.sidebar.show_tree();
            if self.tree_target.is_some() {
                self.refresh_tree_target();
            }
            shown = true;
        }
        if shown {
            self.relayout();
        }
        if self.panes_fit() {
            self.focus = Focus::Tree;
        }
    }

    /// Smallest terminal that can show the requested pane composition.
    ///
    /// Width is 20 main columns, plus the eight-column sidebar when
    /// requested. Height includes the status row and optional app bar.
    /// The pane area needs three main rows, two File-list rows, four
    /// Thread-list rows, or six rows when both lists are requested.
    #[must_use]
    pub(crate) fn minimum_pane_size(&self) -> (usize, usize) {
        let width = super::TEXT_MIN_WIDTH + usize::from(self.sidebar.shown()) * SIDEBAR_MIN_WIDTH;
        let sidebar_rows = match (self.sidebar.tree, self.sidebar.threads) {
            (true, true) => FILE_LIST_MIN_ROWS + THREAD_LIST_MIN_ROWS,
            (true, false) => FILE_LIST_MIN_ROWS,
            (false, true) => THREAD_LIST_MIN_ROWS,
            (false, false) => 0,
        };
        let pane_rows = MAIN_MIN_ROWS.max(sidebar_rows);
        let chrome_rows = 1 + usize::from(self.menu_bar.shown());
        (width, pane_rows + chrome_rows)
    }

    /// Whether the requested pane composition fits the terminal.
    #[must_use]
    pub(crate) fn panes_fit(&self) -> bool {
        let (width, height) = self.minimum_pane_size();
        self.width >= width && self.height >= height
    }

    /// Whether `pane` currently owns unobstructed navigation.
    #[must_use]
    pub(crate) fn pane_has_navigation(&self, pane: Focus) -> bool {
        if self.focus != pane
            || !self.panes_fit()
            || self.title_menu_open()
            || self.popup.is_some()
            || !self.prefix.is_empty()
            || matches!(self.view().mode(), Mode::Command | Mode::Search { .. })
        {
            return false;
        }
        match pane {
            Focus::View => !self.review_list.is_open(),
            Focus::Tree => self.sidebar.tree,
            Focus::Review => self.review_list.is_open(),
            Focus::ThreadsPane => self.sidebar.threads,
        }
    }
}

#[cfg(test)]
mod tests;
