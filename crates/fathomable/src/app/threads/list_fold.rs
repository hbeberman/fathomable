// @okf-doc: /decisions/0076-threads-fold-in-the-list.md
//! The review list's folds (ADR 0076): every thread folds and expands
//! as it does in the text, and a file folds to its row (ADR 0066).
//!
//! `z` acts on the row the cursor is on: a thread's rows fold or expand
//! the thread, a file row folds or unfolds the file. `Z` folds every
//! listed thread when any is expanded and expands them all otherwise,
//! the case rule of ADR 0065; it leaves the file folds alone, and the
//! list has no fold-all for files. The list opens with every thread
//! expanded, and what the reader folds stays folded while the viewer
//! runs. The threads pane keeps its own file folds and has no thread
//! fold.

use std::path::Path;

use fathomable_core::annotations::ThreadId;

use crate::app::App;
use crate::app::threads::list::{ReviewList, Stop};

impl ReviewList {
    /// Whether `path` is folded to its row.
    pub(crate) fn is_folded(&self, path: &Path) -> bool {
        self.folded.contains(path)
    }

    /// Whether `id` is folded to one row.
    pub(crate) fn is_thread_folded(&self, id: &ThreadId) -> bool {
        self.folded_threads.contains(id)
    }
}

impl App {
    /// `z`: fold or expand the cursor's thread; on a file row, fold the
    /// file to its row or unfold it.
    pub(crate) fn review_fold(&mut self) {
        let rows = self.review_rows(self.column_width());
        let Some(index) = self.selected_index(&rows) else {
            return;
        };
        match self.cursor_stop(&rows, index) {
            Stop::File(path) => self.review_toggle_fold(&path),
            Stop::Entry(entry) => {
                let id = rows.entries[entry].id().clone();
                self.review_toggle_thread(&id);
            }
        }
    }

    /// `Z`: fold every listed thread when any is expanded, else expand
    /// them all.
    pub(crate) fn review_fold_all(&mut self) {
        let rows = self.review_rows(self.column_width());
        let listed: Vec<ThreadId> = rows
            .entries
            .iter()
            .map(|entry| entry.id().clone())
            .collect();
        let folded = &mut self.review_list.folded_threads;
        if listed.iter().any(|id| !folded.contains(id)) {
            folded.extend(listed);
        } else {
            for id in &listed {
                folded.remove(id);
            }
        }
        self.review_follow_cursor();
    }

    /// Fold `id` to one row, or expand it again.
    pub(crate) fn review_toggle_thread(&mut self, id: &ThreadId) {
        if !self.review_list.folded_threads.remove(id) {
            self.review_list.folded_threads.insert(id.clone());
        }
        self.review_follow_cursor();
    }

    /// Fold `path` to its row, or unfold it, the cursor resting on the
    /// row.
    pub(crate) fn review_toggle_fold(&mut self, path: &Path) {
        if !self.review_list.folded.remove(path) {
            self.review_list.folded.insert(path.to_path_buf());
        }
        let rows = self.review_rows(self.column_width());
        self.rest_on_file(&rows, path);
    }
}
