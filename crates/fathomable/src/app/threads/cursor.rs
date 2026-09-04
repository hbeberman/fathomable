// @okf-doc: /decisions/0046-one-thread-cursor.md
//! The thread cursor: the one thread and message the reader is on, which
//! the thread pane, the threads pane, and the thread list all show
//! and all move.
//!
//! While the thread pane or the thread list is open the cursor is what
//! the last motion, click, `c`, or `Space a` set, and it stays so while
//! the text cursor rests on the row that motion left it on. Once the
//! reader moves in the text with both closed, the cursor rides the text
//! cursor: the thread starting on the cursor line, else the first on its
//! row, else the nearest starting above, at its newest message, so
//! reading a file walks the threads pane. Every motion resolves the
//! cursor with [`App::thread_cursor`] and steps from it, so a step from
//! any surface is a step from the same place, and two threads folded
//! into one rendered row are still told apart.
//!
//! The motions come in two sizes: [`App::thread_step_in_file`] walks the
//! open file's threads in line order, [`App::thread_step_across`] walks
//! the workspace's, files in path order, opening the file it lands in.
//! Both wrap. The actions — reply, edit, resolve, delete, open — act on
//! the cursor wherever the keys came from.

use std::path::PathBuf;

use fathomable_core::annotations::{MessageTarget, ThreadId};

use crate::app::threads::{ComposeTarget, message_target};
use crate::app::{App, Focus};

/// A thread and a message in it: zero for the opening comment, then the
/// replies in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadCursor {
    thread: Option<ThreadId>,
    message: usize,
}

impl ThreadCursor {
    /// A cursor on `thread` at message `message`.
    #[must_use]
    pub fn new(thread: ThreadId, message: usize) -> Self {
        Self {
            thread: Some(thread),
            message,
        }
    }

    /// The thread the cursor is on, or `None` when there are no threads
    /// to be on.
    #[must_use]
    pub fn thread(&self) -> Option<&ThreadId> {
        self.thread.as_ref()
    }

    /// The highlighted message: zero for the comment, then the replies.
    #[must_use]
    pub fn message(&self) -> usize {
        self.message
    }

    /// The highlighted message as the store names it.
    #[must_use]
    pub fn target(&self) -> MessageTarget {
        message_target(self.message)
    }
}

impl App {
    /// The cursor as every thread surface shows it: the stored one while
    /// the thread pane or the list is open or the text cursor has not
    /// moved since it was set, else the thread under the text cursor at
    /// its newest message.
    #[must_use]
    pub fn thread_cursor(&self) -> ThreadCursor {
        if self.cursor_is_stored() {
            self.thread_cursor.clone()
        } else {
            self.cursor_from_text()
        }
    }

    /// Whether the stored cursor is authoritative: a thread surface that
    /// keeps its own place is open, or the text cursor still rests where
    /// the last thread motion left it.
    fn cursor_is_stored(&self) -> bool {
        self.thread.is_some()
            || self.list.is_open()
            || self.thread_cursor_anchor == Some(self.text_anchor())
    }

    /// Where the text cursor is, for telling a rest from a move.
    fn text_anchor(&self) -> (Option<usize>, usize) {
        (self.current, self.view().cursor().row)
    }

    /// Remember where the text cursor was when the cursor was set.
    fn pin_thread_cursor(&mut self, cursor: ThreadCursor) {
        self.thread_cursor = cursor;
        self.thread_cursor_anchor = Some(self.text_anchor());
    }

    /// The thread the open pane shows: the cursor's, while there is a pane.
    pub(crate) fn pane_thread(&self) -> Option<&ThreadId> {
        self.thread.as_ref().and(self.thread_cursor.thread())
    }

    /// The thread the text cursor is at: among the threads on its row,
    /// the first (in line order) starting on the cursor line, else the
    /// first in line order; with none on the row, the nearest thread
    /// starting above, else the first of the file. Ranges overlap, so
    /// "first on the row" alone would pick a long earlier thread over the
    /// one starting under the cursor.
    fn cursor_from_text(&self) -> ThreadCursor {
        // On an expanded thread's rows the message under the cursor is
        // the cursor's (ADR 0049).
        if let Some((id, message)) = self.expanded_row_message(self.view().cursor().row) {
            return ThreadCursor::new(id, message);
        }
        let order = self.file_threads();
        let Some(first) = order.first() else {
            return ThreadCursor::default();
        };
        let line = self.view().cursor_source_line().unwrap_or(0);
        let position = |id: &ThreadId| order.iter().position(|other| other == id);
        let at_cursor = self.threads_at_cursor();
        let mut candidates: Vec<usize> = at_cursor.iter().filter_map(position).collect();
        candidates.sort_unstable();
        let starts_here = candidates.iter().copied().find(|&index| {
            self.marks()
                .iter()
                .any(|mark| *mark.id() == order[index] && mark.range().start() == line)
        });
        let index = starts_here
            .or_else(|| candidates.first().copied())
            .or_else(|| {
                self.marks()
                    .iter()
                    .filter(|mark| mark.range().start() < line)
                    .max_by_key(|mark| mark.range().start())
                    .and_then(|mark| position(mark.id()))
            });
        let id = index.map_or(first, |index| &order[index]);
        ThreadCursor::new(id.clone(), self.newest_message(id))
    }

    /// The index of a thread's newest message.
    pub(crate) fn newest_message(&self, id: &ThreadId) -> usize {
        self.thread(id).map_or(0, |thread| thread.replies().len())
    }

    /// Messages in the cursor's thread.
    pub(crate) fn cursor_message_count(&self) -> usize {
        self.thread_cursor()
            .thread()
            .and_then(|id| self.thread(id))
            .map_or(0, |thread| thread.replies().len() + 1)
    }

    /// Put the cursor on `id`, at its newest message unless the cursor
    /// already stands on it.
    pub(crate) fn set_thread_cursor(&mut self, id: ThreadId) {
        let current = self.thread_cursor();
        let message = if current.thread() == Some(&id) {
            current.message()
        } else {
            self.newest_message(&id)
        };
        self.pin_thread_cursor(ThreadCursor::new(id, message));
    }

    /// Put the cursor on message `message` of `id`, clamped to the
    /// thread's messages.
    pub(crate) fn set_thread_cursor_message(&mut self, id: ThreadId, message: usize) {
        let last = self.newest_message(&id);
        self.pin_thread_cursor(ThreadCursor::new(id, message.min(last)));
    }

    // ----- motions -----

    /// Where the cursor stands in `order`: `Ok(i)` on thread `i`, or
    /// `Err(i)` between threads `i - 1` and `i` when the text cursor is
    /// driving and sits on no thread's first line.
    fn anchor_in(&self, order: &[ThreadId]) -> Result<usize, usize> {
        if self.cursor_is_stored()
            && let Some(index) = self
                .thread_cursor
                .thread()
                .and_then(|id| order.iter().position(|other| other == id))
        {
            return Ok(index);
        }
        let here = (
            self.current_path().to_path_buf(),
            self.view().cursor_source_line().unwrap_or(0),
        );
        if let Some(index) = order
            .iter()
            .position(|id| self.thread_start(id).as_ref() == Some(&here))
        {
            return Ok(index);
        }
        Err(order
            .iter()
            .position(|id| self.thread_start(id).is_some_and(|start| start > here))
            .unwrap_or(order.len()))
    }

    /// `(path, first line)` of `id`: from the loaded document's mark when
    /// its file is open, else as stored.
    fn thread_start(&self, id: &ThreadId) -> Option<(PathBuf, usize)> {
        let thread = self.thread(id)?;
        let line = self
            .docs
            .iter()
            .find(|doc| doc.relative == thread.path())
            .and_then(|doc| doc.marks.iter().find(|mark| mark.id() == id))
            .map_or_else(|| thread.range().start(), |mark| mark.range().start());
        Some((thread.path().to_path_buf(), line))
    }

    /// The thread `delta` steps from the cursor in `order`, wrapping, with
    /// a notice when it wrapped.
    pub(super) fn step_in(&mut self, order: &[ThreadId], delta: isize) -> Option<ThreadId> {
        let len = order.len().cast_signed();
        if len == 0 {
            return None;
        }
        // Between threads, a forward step lands on the next one rather
        // than skipping it.
        let from = match self.anchor_in(order) {
            Err(index) if delta > 0 => index.cast_signed() + delta - 1,
            Ok(index) | Err(index) => index.cast_signed() + delta,
        };
        if from < 0 {
            self.notice("wrapped to last thread");
        } else if from >= len {
            self.notice("wrapped to first thread");
        }
        Some(order[from.rem_euclid(len).cast_unsigned()].clone())
    }

    /// `]c` / `[c` in the text, `l` / `h` in the pane: the next or
    /// previous thread of this file.
    pub fn thread_step_in_file(&mut self, delta: isize) {
        let order = self.file_threads();
        if order.is_empty() {
            self.notice("no threads in this file");
            return;
        }
        if let Some(id) = self.step_in(&order, delta) {
            self.land_on_thread(id);
        }
    }

    /// `]C` / `[C` in the text, `L` / `H` in the pane: the next or
    /// previous thread across the workspace, opening its file.
    pub fn thread_step_across(&mut self, delta: isize) {
        let order = self.workspace_threads();
        if order.is_empty() {
            self.notice("no threads in the workspace");
            return;
        }
        if let Some(id) = self.step_in(&order, delta) {
            self.land_on_thread(id);
        }
    }

    /// Whether the cursor is on the first thread of the file, where `h`
    /// in the pane hops to the threads pane instead of wrapping.
    pub(crate) fn cursor_on_first_in_file(&self) -> bool {
        let order = self.file_threads();
        self.anchor_in(&order) == Ok(0)
    }

    /// Go to `id`: its file opened when it is elsewhere, the text cursor
    /// on its first line, the cursor on it, and the pane showing it when
    /// the pane is open. A deleted file is reported instead.
    pub(crate) fn land_on_thread(&mut self, id: ThreadId) -> bool {
        let Some(path) = self.thread(&id).map(|thread| thread.path().to_path_buf()) else {
            return false;
        };
        // Showing another file hands the keys to the text; a pane that
        // was open keeps them where they were.
        let focus = self.focus;
        if path != self.current_path() {
            if !self.workspace.root().join(&path).is_file() {
                self.notice(format!("{} is deleted", path.display()));
                return false;
            }
            self.close_popup();
            self.open(&path);
        }
        self.goto_thread(&id);
        self.set_thread_cursor(id.clone());
        if self.thread.is_some() {
            self.open_thread(id);
            self.focus = focus;
        }
        true
    }

    /// `j` / `k` in the pane and the list: the next or previous message
    /// of the cursor's thread, without wrapping.
    pub fn message_step(&mut self, delta: isize) {
        let count = self.cursor_message_count();
        if count == 0 {
            return;
        }
        let cursor = self.thread_cursor();
        let message = cursor.message().saturating_add_signed(delta).min(count - 1);
        self.go_to_message(message);
    }

    /// `gg` in the pane: the opening comment.
    pub fn message_first(&mut self) {
        if self.cursor_message_count() > 0 {
            self.go_to_message(0);
        }
    }

    /// `ge` / `G` in the pane: the newest message.
    pub fn message_last(&mut self) {
        let count = self.cursor_message_count();
        if count > 0 {
            self.go_to_message(count - 1);
        }
    }

    fn go_to_message(&mut self, message: usize) {
        if let Some(id) = self.thread_cursor().thread().cloned() {
            self.set_thread_cursor_message(id, message);
            self.follow_cursor_message();
        }
    }

    /// Keep the highlighted message on screen in whichever surface shows
    /// it.
    pub(crate) fn follow_cursor_message(&mut self) {
        if self.thread.is_some() {
            self.thread_message_into_view();
        }
        if self.list.is_open() {
            self.thread_list_follow_cursor();
        }
    }

    // ----- actions on the cursor -----

    /// `r`: reply to the cursor's thread through the comment box.
    pub fn thread_reply(&mut self) {
        match self.thread_cursor().thread().cloned() {
            Some(id) => self.open_compose(ComposeTarget::Reply(id)),
            None => self.notice("no thread here"),
        }
    }

    /// `e`: edit the highlighted message when the user wrote it.
    pub fn thread_edit_message(&mut self) {
        let cursor = self.thread_cursor();
        if let Some(id) = cursor.thread().cloned() {
            self.open_compose(ComposeTarget::Edit {
                thread: id,
                message: cursor.target(),
            });
        }
    }

    /// `Space c e`: edit the newest message of the cursor's thread that
    /// the user wrote, wherever the highlight is (ADR 0049). The comment
    /// is always the user's.
    pub fn thread_edit_newest_own(&mut self) {
        let Some(id) = self.thread_cursor().thread().cloned() else {
            self.notice("no thread here");
            return;
        };
        let Some(thread) = self.thread(&id) else {
            return;
        };
        let newest = thread
            .replies()
            .iter()
            .rposition(|reply| reply.author().is_user())
            .map_or(0, |index| index + 1);
        self.set_thread_cursor_message(id.clone(), newest);
        self.open_compose(ComposeTarget::Edit {
            thread: id,
            message: message_target(newest),
        });
    }

    /// Whether the highlighted message belongs to the user.
    #[must_use]
    pub fn thread_message_editable(&self) -> bool {
        let cursor = self.thread_cursor();
        cursor
            .thread()
            .and_then(|id| self.message_for(id, cursor.target()))
            .is_some_and(|(_, editable)| editable)
    }

    /// `o`: resolve the cursor's thread, or reopen it.
    pub fn thread_toggle_resolved(&mut self) {
        let Some(id) = self.thread_cursor().thread().cloned() else {
            self.notice("no thread here");
            return;
        };
        self.toggle_resolved(&id);
        if self.list.is_open() {
            self.thread_list_follow_cursor();
        }
    }

    /// The first `d`: arm deletion of the cursor's thread (ADR 0034).
    pub fn thread_arm_delete(&mut self) {
        if let Some(id) = self.thread_cursor().thread().cloned() {
            self.arm_delete(id);
        }
    }

    /// Enter in the list: open the file and the pane on the highlighted
    /// message, the list closing as the document takes the column.
    pub fn thread_open_in_file(&mut self) {
        let cursor = self.thread_cursor();
        let Some(id) = cursor.thread().cloned() else {
            return;
        };
        self.close_thread_list();
        if self.land_on_thread(id.clone()) {
            self.open_thread(id.clone());
            self.set_thread_cursor_message(id, cursor.message());
            self.thread_message_into_view();
        }
    }

    /// Enter or `l` in the threads pane: the pane on the cursor's
    /// thread takes the keys.
    pub fn focus_thread_pane(&mut self) {
        if let Some(id) = self.thread_cursor().thread().cloned() {
            self.show_thread(id);
            self.focus = Focus::Thread;
        }
    }

    /// Jump to `id` in the open document and open the pane on it.
    pub(crate) fn show_thread(&mut self, id: ThreadId) {
        self.goto_thread(&id);
        self.open_thread(id);
    }
}
