// @okf-doc: /decisions/0090-direct-workspace-navigation.md
//! Navigation-owned visibility that does not change persistent folds.

use std::path::{Path, PathBuf};

use fathomable_core::annotations::ThreadId;

use crate::app::App;

#[derive(Debug, Default)]
pub(crate) struct NavigationPeek {
    file_thread: Option<ThreadId>,
    file_message: Option<usize>,
    review_thread: Option<ThreadId>,
    review_message: Option<usize>,
    review_file: Option<PathBuf>,
    pane_file: Option<PathBuf>,
}

impl App {
    pub(crate) fn dismiss_navigation_peek(&mut self) {
        self.change_stop = None;
        self.change_placement = None;
        if self.navigation_peek.file_thread.is_some() {
            self.navigation_peek.file_thread = None;
            self.navigation_peek.file_message = None;
            self.place_stub_rows();
        }
        self.navigation_peek.review_thread = None;
        self.navigation_peek.review_message = None;
        self.navigation_peek.review_file = None;
        self.navigation_peek.pane_file = None;
    }

    /// Reapply a deliberate thread or comparison placement after passive
    /// resize or relayout changed rendered row coordinates.
    pub(crate) fn restore_navigation_placement(&mut self) {
        if self.draft().is_some() {
            if self.review_list().is_open() {
                self.review_follow_draft();
            } else if let Some((row, _)) = self.draft_cursor_cell() {
                self.view_mut().reveal_row(row, 0);
            }
            return;
        }
        if self.review_list().is_open()
            && let Some(id) = self.navigation_peek.review_thread.clone()
        {
            let newest = self.newest_message(&id);
            let message = self
                .navigation_peek
                .review_message
                .unwrap_or(newest)
                .min(newest);
            self.navigation_peek.review_message = Some(message);
            self.place_review_thread_at(&id, message);
            return;
        }
        if !self.review_list().is_open()
            && let Some(id) = self.navigation_peek.file_thread.clone()
        {
            let Some(message) = self.navigation_peek.file_message else {
                return;
            };
            self.seat_file_message(id.clone(), message);
            if let Some((span, priority)) = self.file_thread_jump_span(&id, message) {
                self.view_mut().place_jump_span(span, priority);
            }
            self.set_file_thread_cursor(id, message);
            return;
        }
        self.restore_change_jump_placement();
    }

    /// Remember whether a temporary File reveal currently seats a message
    /// or ordinary source before a passive relayout changes row identities.
    pub(crate) fn capture_file_navigation_seat(&mut self) {
        let Some(id) = self.navigation_peek.file_thread.clone() else {
            return;
        };
        self.navigation_peek.file_message = self
            .expanded_row_message(self.view().cursor().row)
            .and_then(|(seated, message)| (seated == id).then_some(message));
    }

    pub(crate) fn peek_file_thread(&mut self, id: ThreadId) {
        self.dismiss_navigation_peek();
        self.navigation_peek.file_message = Some(self.newest_message(&id));
        self.navigation_peek.file_thread = Some(id);
        self.place_stub_rows();
    }

    pub(super) fn peek_review_thread(&mut self, id: ThreadId, path: PathBuf) {
        self.dismiss_navigation_peek();
        self.navigation_peek.review_message = Some(self.newest_message(&id));
        self.navigation_peek.review_thread = Some(id);
        self.navigation_peek.review_file = Some(path);
    }

    pub(super) fn peek_pane_file(&mut self, path: PathBuf) {
        self.navigation_peek.pane_file = Some(path);
    }

    pub(super) fn file_thread_peeked(&self, id: &ThreadId) -> bool {
        self.navigation_peek.file_thread.as_ref() == Some(id)
    }

    pub(super) fn update_file_peek_message(&mut self, id: &ThreadId, message: usize) {
        if self.file_thread_peeked(id) {
            self.navigation_peek.file_message = Some(message);
        }
    }

    pub(super) fn review_thread_peeked(&self, id: &ThreadId) -> bool {
        self.navigation_peek.review_thread.as_ref() == Some(id)
    }

    pub(super) fn review_peek_message(&self, id: &ThreadId) -> Option<usize> {
        self.review_thread_peeked(id)
            .then_some(self.navigation_peek.review_message)
            .flatten()
            .map(|message| message.min(self.newest_message(id)))
    }

    pub(super) fn activate_review_peek_cursor(&mut self) {
        let Some(id) = self.navigation_peek.review_thread.clone() else {
            return;
        };
        let message = self
            .navigation_peek
            .review_message
            .unwrap_or_else(|| self.newest_message(&id));
        self.set_review_thread_cursor(id, message);
    }

    pub(crate) fn update_review_peek_message(&mut self, id: &ThreadId, message: usize) {
        if self.review_thread_peeked(id) {
            self.navigation_peek.review_message = Some(message.min(self.newest_message(id)));
        }
    }

    pub(super) fn review_file_peeked(&self, path: &Path) -> bool {
        self.navigation_peek.review_file.as_deref() == Some(path)
    }

    pub(super) fn pane_file_peeked(&self, path: &Path) -> bool {
        self.navigation_peek.pane_file.as_deref() == Some(path)
    }

    pub(super) fn release_file_peek(&mut self, id: &ThreadId) {
        if self.file_thread_peeked(id) {
            self.navigation_peek.file_thread = None;
            self.navigation_peek.file_message = None;
        }
    }

    /// Retire comparison-owned placement after ordinary File movement.
    pub(crate) fn retire_change_jump(&mut self) {
        self.change_placement = None;
    }

    pub(super) fn release_review_thread_peek(&mut self, id: &ThreadId) {
        if self.review_thread_peeked(id) {
            self.navigation_peek.review_thread = None;
            self.navigation_peek.review_message = None;
        }
    }

    pub(super) fn release_review_file_peek(&mut self, path: &Path) {
        if self.review_file_peeked(path) {
            self.navigation_peek.review_file = None;
        }
    }

    pub(super) fn release_pane_file_peek(&mut self, path: &Path) {
        if self.pane_file_peeked(path) {
            self.navigation_peek.pane_file = None;
        }
    }

    pub(super) fn release_all_review_thread_peeks(&mut self) {
        self.navigation_peek.review_thread = None;
        self.navigation_peek.review_message = None;
    }

    pub(super) fn release_all_pane_file_peeks(&mut self) {
        self.navigation_peek.pane_file = None;
    }
}
