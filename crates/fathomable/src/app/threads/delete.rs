// @okf-doc: /decisions/0034-deleting-threads.md
//! Deleting a thread with `d d` (ADR 0034): the first `d` arms, the
//! second deletes, any other key cancels and is swallowed.
//!
//! The arming is one field holding the thread id beside the key prefix,
//! not a mode: a store reload that removes the thread between the two
//! keys makes the second `d` a no-op, and the three surfaces that offer
//! `d` share one step.

use fathomable_core::annotations::ThreadId;

use crate::app::{App, Focus};
use fathomable_core::clock::now;

const ARMED: &str = "d again to delete this thread · any other key cancels";

impl App {
    /// The thread a first `d` armed for deletion.
    #[cfg(test)]
    pub(crate) fn delete_armed(&self) -> Option<&ThreadId> {
        self.pending_delete.as_ref()
    }

    /// `d` on a surface showing `id`: arm its deletion.
    pub(crate) fn arm_delete(&mut self, id: ThreadId) {
        self.pending_delete = Some(id);
        self.notice(ARMED);
    }

    /// The first `d` on a thread surface: arm the cursor's thread.
    pub(crate) fn arm_delete_here(&mut self) {
        match self.focus() {
            Focus::ThreadsPane | Focus::Review => self.thread_arm_delete(),
            // In the text, only a thread covering the cursor row is armed
            // (ADR 0049), never one further up the file.
            Focus::View if !self.threads_at_cursor().is_empty() => self.thread_arm_delete(),
            Focus::View | Focus::Tree => {}
        }
    }

    /// The second `d`: delete the armed thread, if one still is.
    pub(crate) fn delete_armed_thread(&mut self) {
        if let Some(id) = self.pending_delete.take() {
            self.delete_thread(&id);
        }
    }

    /// `Space c d`: delete the cursor's thread outright (ADR 0049).
    pub(crate) fn thread_delete_here(&mut self) {
        match self.thread_cursor().thread().cloned() {
            Some(id) => self.delete_thread(&id),
            None => self.notice("no thread here"),
        }
    }

    /// A click while armed cancels; the click is then handled.
    pub(crate) fn cancel_delete(&mut self) {
        if self.pending_delete.take().is_some() {
            self.notice("delete cancelled");
        }
    }

    /// Delete `id`: a tombstone in the store, every document's marks
    /// refreshed, and its rows gone from the text.
    pub(crate) fn delete_thread(&mut self, id: &ThreadId) {
        let place = self.review_selected_index();
        let Some(store) = self.store_mut() else {
            return;
        };
        let result = store.delete(id, now());
        self.refresh_after_thread_store_change();
        if let Err(error) = result {
            self.notice(format!("cannot delete thread: {error}"));
            return;
        }
        tracing::info!(%id, "thread deleted");
        // An expanded thread that is gone leaves its rows with it, and a
        // cursor pinned on it rides the text again (ADR 0046).
        self.expanded.remove(id);
        if self.thread_cursor.thread() == Some(id) {
            self.thread_cursor = crate::app::threads::cursor::ThreadCursor::default();
            self.thread_cursor_anchor = None;
        }
        self.place_stub_rows();
        if self.focus == Focus::ThreadsPane && self.threads_pane_height() == 0 {
            self.focus = self.displayed_main_focus();
        }
        self.review_reselect(place);
        self.notice("deleted");
    }
}

#[cfg(test)]
mod tests {

    use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use fathomable_core::annotations::Store;

    use crate::app::{App, Focus};

    use crate::app::testing::{self, press_key, source_app};

    fn annotate(app: &mut App, line: usize, text: &str) {
        app.view_mut().goto_source_line(line);
        app.start_new_comment();
        app.compose_insert(text);
        app.compose_submit();
    }

    #[test]
    fn d_d_deletes_from_each_surface_and_any_other_key_cancels() -> anyhow::Result<()> {
        let dir = testing::workspace("delete-surfaces", testing::README)?;
        let mut app = source_app(&dir)?;
        app.show_tree();
        annotate(&mut app, 3, "three");
        annotate(&mut app, 5, "five");
        annotate(&mut app, 7, "seven");
        assert_eq!(app.marks().len(), 3);

        // The text: a cancel is swallowed, the second `d` deletes, and
        // the cursor stays where it was with the next thread as its own.
        app.focus_pane(Focus::View);
        app.view_mut().goto_source_line(3);
        app.expand_at_cursor();
        assert_eq!(app.focus(), Focus::View);
        press_key(&mut app, KeyCode::Char('d'));
        assert!(app.delete_armed().is_some());
        assert!(app.message().is_some_and(|m| m.starts_with("d again")));
        press_key(&mut app, KeyCode::Char('n'));
        assert!(app.delete_armed().is_none());
        assert_eq!(app.message(), Some("delete cancelled"));
        assert_eq!(app.thread_position(), Some((1, 3)), "the `n` was swallowed");
        press_key(&mut app, KeyCode::Char('d'));
        press_key(&mut app, KeyCode::Char('d'));
        assert_eq!(app.marks().len(), 2);
        assert_eq!(app.thread_position(), Some((1, 2)));
        assert_eq!(app.view().cursor_source_line(), Some(3));
        assert_eq!(app.message(), Some("deleted"));

        // The threads pane: `d d` on the highlight; a click cancels.
        app.focus_threads_pane();
        press_key(&mut app, KeyCode::Char('d'));
        crate::app::input::mouse::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 50,
                row: 3,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(app.delete_armed().is_none());
        assert_eq!(app.marks().len(), 2);
        // The click left the text cursor on L4, between the threads, so
        // `j` in the pane lands on the next one (ADR 0046).
        app.focus_threads_pane();
        press_key(&mut app, KeyCode::Char('j'));
        assert_eq!(app.view().cursor_source_line(), Some(5));
        press_key(&mut app, KeyCode::Char('j'));
        assert_eq!(app.view().cursor_source_line(), Some(7));
        press_key(&mut app, KeyCode::Char('d'));
        press_key(&mut app, KeyCode::Char('d'));
        assert_eq!(app.marks().len(), 1);
        assert_eq!(app.marks()[0].range().map(|r| r.start()), Some(5));
        assert_eq!(app.focus(), Focus::ThreadsPane, "one thread left");

        // The review list: the entry under the selection goes.
        app.open_review();
        assert_eq!(app.focus(), Focus::Review);
        press_key(&mut app, KeyCode::Char('d'));
        press_key(&mut app, KeyCode::Char('d'));
        assert!(app.marks().is_empty());
        assert!(app.review_rows(80).entries.is_empty());
        app.close_review();

        // The store agrees after a reload.
        let reloaded = Store::open(dir.0.join("state/threads.jsonl"))?;
        assert!(reloaded.threads().is_empty());
        Ok(())
    }

    #[test]
    fn deleting_the_last_thread_leaves_the_text_clean() -> anyhow::Result<()> {
        let dir = testing::workspace("delete-last", testing::README)?;
        let mut app = source_app(&dir)?;
        annotate(&mut app, 3, "only");
        app.expand_at_cursor();
        press_key(&mut app, KeyCode::Char('d'));
        press_key(&mut app, KeyCode::Char('d'));
        assert!(!app.shows_thread());
        assert_eq!(app.focus(), Focus::View);
        Ok(())
    }
}
