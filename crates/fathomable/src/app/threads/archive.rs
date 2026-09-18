// @okf-doc: /decisions/0087-global-comparisons-and-board-history.md
//! Repository-wide archive, restore, and clear-board actions.

use fathomable_core::annotations::{ArchiveContext, Author, OriginVersion, Status, ThreadId};
use fathomable_core::clock::now;

use crate::app::{App, Popup};

/// Counts captured by the clear-board confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BoardCounts {
    pub(crate) open: usize,
    pub(crate) resolved: usize,
}

impl App {
    /// Archive every thread that is still resolved under the store lock.
    pub(crate) fn archive_resolved_threads(&mut self) {
        let context = self.archive_context();
        let Some(store) = self.store_mut() else {
            return;
        };
        let result = store.archive_resolved_with_context(&context);
        match result {
            Ok(ids) if ids.is_empty() => self.notice("no resolved threads to archive"),
            Ok(ids) => {
                self.refresh_all_marks();
                self.reshow_review();
                self.notice(format!("archived {} resolved thread(s)", ids.len()));
            }
            Err(error) => self.notice(format!("cannot archive resolved threads: {error}")),
        }
    }

    /// Open the repository-wide clear-board confirmation.
    pub(crate) fn request_clear_board(&mut self) {
        let Some(store) = self.store.as_ref() else {
            self.store_mut();
            return;
        };
        let slate = store.board_slate();
        if slate.entries().is_empty() {
            self.notice("board is empty");
            return;
        }
        let counts = slate_counts(store);
        self.popup = Some(Popup::ConfirmBoard {
            slate,
            counts,
            changed: false,
        });
    }

    /// Confirm the previously acknowledged clear-board slate.
    pub(crate) fn confirm_clear_board(&mut self) {
        let Some(Popup::ConfirmBoard { slate, .. }) = self.popup.take() else {
            return;
        };
        let context = self.archive_context();
        let Some(store) = self.store_mut() else {
            return;
        };
        let result = store.clear_board_with_context(&slate, &context);
        match result {
            Ok(ids) => {
                self.refresh_all_marks();
                self.reshow_review();
                self.notice(format!("cleared board; archived {} thread(s)", ids.len()));
            }
            Err(error) if error.is_slate_changed() => {
                let slate = store.board_slate();
                if slate.entries().is_empty() {
                    self.notice("board changed and is now empty");
                    return;
                }
                let counts = slate_counts(store);
                self.popup = Some(Popup::ConfirmBoard {
                    slate,
                    counts,
                    changed: true,
                });
            }
            Err(error) => self.notice(format!("cannot clear board: {error}")),
        }
    }

    /// Cancel a clear-board confirmation without touching the store.
    pub(crate) fn cancel_clear_board(&mut self) {
        if matches!(self.popup, Some(Popup::ConfirmBoard { .. })) {
            self.popup = None;
            self.notice("clear board cancelled");
        }
    }

    /// Archive one selected thread without changing its lifecycle.
    pub(crate) fn archive_thread(&mut self, id: &ThreadId) {
        if self
            .thread(id)
            .is_some_and(|thread| thread.status() != Status::Resolved)
        {
            self.notice("resolve the thread before archiving it");
            return;
        }
        let pane_place = (self.focus() == crate::app::Focus::ThreadsPane)
            .then(|| self.threads_pane_selected())
            .flatten();
        let context = self.archive_context();
        let Some(store) = self.store_mut() else {
            return;
        };
        let result = store.archive_with_context(id, context);
        match result {
            Ok(()) => {
                self.refresh_all_marks();
                self.reshow_review();
                if self.focus() == crate::app::Focus::ThreadsPane {
                    self.threads_pane_reselect(pane_place);
                }
                self.notice("thread archived");
            }
            Err(error) => self.notice(format!("cannot archive thread: {error}")),
        }
    }

    /// Restore one archived thread without changing its lifecycle.
    pub(crate) fn restore_thread(&mut self, id: &ThreadId) {
        let pane_place = (self.focus() == crate::app::Focus::ThreadsPane)
            .then(|| self.threads_pane_selected())
            .flatten();
        let context = self.archive_context();
        let Some(store) = self.store_mut() else {
            return;
        };
        let result = store.restore_with_context(id, context);
        match result {
            Ok(()) => {
                self.refresh_all_marks();
                self.reshow_review();
                if self.focus() == crate::app::Focus::ThreadsPane {
                    self.threads_pane_reselect(pane_place);
                }
                self.notice("thread restored");
            }
            Err(error) => self.notice(format!("cannot restore thread: {error}")),
        }
    }

    fn archive_context(&self) -> ArchiveContext {
        let version = self
            .workspace()
            .head_commit()
            .map_or_else(|| OriginVersion::working_tree(None), OriginVersion::commit);
        ArchiveContext::new(Author::User, now())
            .at_checkout(self.workspace().root().display().to_string())
            .at_version(version)
    }
}

fn slate_counts(store: &fathomable_core::annotations::Store) -> BoardCounts {
    let mut counts = BoardCounts {
        open: 0,
        resolved: 0,
    };
    for thread in store.threads() {
        match thread.status() {
            Status::Open => counts.open += 1,
            Status::Resolved => counts.resolved += 1,
        }
    }
    counts
}
