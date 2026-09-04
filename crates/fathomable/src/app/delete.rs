// @okf-doc: /decisions/0034-deleting-threads.md
//! Deleting a thread with `d d` (ADR 0034): the first `d` arms, the
//! second deletes, any other key cancels and is swallowed.
//!
//! The arming is one field holding the thread id beside the key prefix,
//! not a mode: a store reload that removes the thread between the two
//! keys makes the second `d` a no-op, and the three surfaces that offer
//! `d` share one step.

use fathomable_core::annotations::ThreadId;

use super::threads::now;
use super::{App, Focus};

const ARMED: &str = "d again to delete this thread · any other key cancels";

impl App {
    /// The thread a first `d` armed for deletion.
    pub fn delete_armed(&self) -> Option<&ThreadId> {
        self.pending_delete.as_ref()
    }

    /// `d` on a surface showing `id`: arm its deletion.
    pub fn arm_delete(&mut self, id: ThreadId) {
        self.pending_delete = Some(id);
        self.notice(ARMED);
    }

    /// The first `d` on a thread surface: arm the cursor's thread.
    pub fn arm_delete_here(&mut self) {
        match self.focus() {
            Focus::Thread | Focus::FileThreads | Focus::Threads => self.thread_arm_delete(),
            Focus::View | Focus::Sidebar => {}
        }
    }

    /// The second `d`: delete the armed thread, if one still is.
    pub fn delete_armed_thread(&mut self) {
        if let Some(id) = self.pending_delete.take() {
            self.delete_thread(&id);
        }
    }

    /// A click while armed cancels; the click is then handled.
    pub fn cancel_delete(&mut self) {
        if self.pending_delete.take().is_some() {
            self.notice("delete cancelled");
        }
    }

    /// Delete `id`: a tombstone in the store, every document's marks
    /// refreshed, and the thread pane moved on when it showed the thread.
    pub fn delete_thread(&mut self, id: &ThreadId) {
        let order = self.file_threads();
        let place = self.thread_list_selected_index();
        let Some(store) = self.store_mut() else {
            return;
        };
        if let Err(error) = store.delete(id, now()) {
            self.notice(format!("cannot delete thread: {error}"));
            return;
        }
        tracing::info!(%id, "thread deleted");
        self.refresh_all_marks();
        if self.thread.is_some() && self.thread_cursor.thread() == Some(id) {
            let next = order
                .iter()
                .position(|other| other == id)
                .and_then(|index| {
                    order[index + 1..]
                        .iter()
                        .chain(&order[..index])
                        .next()
                        .cloned()
                });
            match next {
                Some(next) => {
                    // The surface that deleted keeps the keys.
                    self.goto_thread(&next);
                    self.open_thread_behind(next);
                }
                None => self.close_thread(),
            }
        }
        if self.focus == Focus::FileThreads && self.file_thread_pane_rows() == 0 {
            self.focus = Focus::View;
        }
        self.thread_list_reselect(place);
        self.notice("deleted");
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use fathomable_core::annotations::Store;
    use fathomable_core::workspace::Workspace;

    use crate::app::input::keys;
    use crate::app::{App, Focus, Options};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> std::io::Result<Self> {
            let dir = std::env::temp_dir()
                .join(format!("fathomable-delete-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("ws"))?;
            fs::write(
                dir.join("ws/README.md"),
                "# Readme\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
            )?;
            Ok(Self(dir))
        }

        fn app(&self) -> anyhow::Result<App> {
            let workspace = Workspace::discover(self.0.join("ws"))?;
            let store = Store::open(self.0.join("state/threads.jsonl"))?;
            let options = Options {
                store: Some(store),
                ..Options::for_test(self.0.join("ws"))
            };
            let mut app = App::new(workspace, 100, 30, options);
            app.open(Path::new("README.md"));
            app.view_mut().toggle_source_view();
            Ok(app)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn annotate(app: &mut App, line: usize, text: &str) {
        app.view_mut().goto_source_line(line);
        app.start_new_comment();
        app.compose_insert(text);
        app.compose_submit();
    }

    fn press(app: &mut App, code: KeyCode) {
        keys::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn d_d_deletes_from_each_surface_and_any_other_key_cancels() -> anyhow::Result<()> {
        let dir = TempDir::new("surfaces")?;
        let mut app = dir.app()?;
        app.show_sidebar();
        annotate(&mut app, 3, "three");
        annotate(&mut app, 5, "five");
        annotate(&mut app, 7, "seven");
        assert_eq!(app.marks().len(), 3);

        // The thread pane: a cancel is swallowed, the second `d` deletes
        // and the pane moves to the next thread in the file.
        app.view_mut().goto_source_line(3);
        app.open_thread_at_cursor();
        assert_eq!(app.focus(), Focus::Thread);
        press(&mut app, KeyCode::Char('d'));
        assert!(app.delete_armed().is_some());
        assert!(app.message().is_some_and(|m| m.starts_with("d again")));
        press(&mut app, KeyCode::Char('n'));
        assert!(app.delete_armed().is_none());
        assert_eq!(app.message(), Some("delete cancelled"));
        assert_eq!(app.thread_position(), Some((1, 3)), "the `n` was swallowed");
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.marks().len(), 2);
        assert_eq!(app.thread_position(), Some((1, 2)));
        assert_eq!(app.view().cursor_source_line(), Some(5));
        assert_eq!(app.message(), Some("deleted"));

        // The file-threads pane: `d d` on the highlight; a click cancels.
        app.close_thread();
        app.focus_file_threads();
        press(&mut app, KeyCode::Char('d'));
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
        app.focus_file_threads();
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.view().cursor_source_line(), Some(7));
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.marks().len(), 1);
        assert_eq!(app.marks()[0].range().start(), 5);
        assert_eq!(app.focus(), Focus::FileThreads, "one thread left");

        // The thread list: the entry under the selection goes.
        app.close_thread();
        app.open_thread_list();
        assert_eq!(app.focus(), Focus::Threads);
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        assert!(app.marks().is_empty());
        assert!(app.thread_list_rows(80).entries.is_empty());
        app.close_thread_list();

        // The store agrees after a reload.
        let reloaded = Store::open(dir.0.join("state/threads.jsonl"))?;
        assert!(reloaded.threads().is_empty());
        Ok(())
    }

    #[test]
    fn deleting_the_last_thread_closes_the_pane() -> anyhow::Result<()> {
        let dir = TempDir::new("last")?;
        let mut app = dir.app()?;
        annotate(&mut app, 3, "only");
        app.open_thread_at_cursor();
        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('d'));
        assert!(app.thread_panel().is_none());
        assert_eq!(app.focus(), Focus::View);
        Ok(())
    }
}
