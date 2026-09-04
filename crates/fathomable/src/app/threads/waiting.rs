// @okf-doc: /decisions/0030-waiting-threads.md
//! Threads waiting on the user (ADR 0030).
//!
//! A thread *waits* when it is open and an agent wrote its newest message
//! ([`Thread::awaits`]); the user's reply, resolve, or reopen ends
//! the wait, so nothing is tracked per viewer. This module counts the
//! waiting threads for the status line and the sidebar, raises a toast
//! when a store reload turns a thread waiting, and walks them with
//! `]r` / `[r`: the current document's below (above) the cursor first,
//! then the other files' in path order, wrapping.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use fathomable_core::annotations::{Party, Store, Thread, ThreadId};

use crate::app::App;
use crate::app::threads::ThreadState;

impl App {
    /// Waiting threads on the current document.
    pub fn waiting_count(&self) -> usize {
        self.marks()
            .iter()
            .filter(|mark| mark.kind() == ThreadState::Waiting)
            .count()
    }

    /// Waiting threads across the work in scope.
    pub fn waiting_total(&self) -> usize {
        self.waiting_threads().count()
    }

    /// Whether a thread on `path` (root-relative) waits on the user.
    pub fn path_waits(&self, path: &Path) -> bool {
        self.waiting_threads().any(|thread| thread.path() == path)
    }

    fn waiting_threads(&self) -> impl Iterator<Item = &Thread> + '_ {
        self.store
            .iter()
            .flat_map(Store::threads)
            .filter(|thread| self.reach.includes(thread) && thread.awaits(Party::User))
    }

    /// The ids of the store's waiting threads, for the reload diff.
    pub(crate) fn waiting_ids(store: &Store) -> HashSet<ThreadId> {
        store
            .threads()
            .iter()
            .filter(|thread| thread.awaits(Party::User))
            .map(|thread| thread.id().clone())
            .collect()
    }

    /// After a store reload: toast the threads that started waiting, so
    /// an agent's reply is seen wherever the reader is.
    pub(crate) fn toast_waiting(&mut self, before: &HashSet<ThreadId>) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let arrived: Vec<String> = store
            .threads()
            .iter()
            .filter(|thread| thread.awaits(Party::User) && !before.contains(thread.id()))
            .map(|thread| format!("{}:{}", thread.path().display(), thread.range().start()))
            .collect();
        let text = match arrived.as_slice() {
            [] => return,
            [one] => format!("reply on {one}"),
            many => format!("{} replies", many.len()),
        };
        tracing::info!(count = arrived.len(), "replies arrived");
        self.push_toast(text);
    }

    /// `]r`: the next waiting thread, across files, and its pane.
    pub fn waiting_next(&mut self) {
        self.step_waiting(true);
    }

    /// `[r`: the previous waiting thread, across files, and its pane.
    pub fn waiting_prev(&mut self) {
        self.step_waiting(false);
    }

    fn step_waiting(&mut self, forward: bool) {
        // First the current document, beyond the cursor — or beyond the
        // thread whose pane is open, since a rendered row can hold
        // several source lines and the cursor alone cannot tell them
        // apart.
        let here = self.waiting_marks_here();
        let shown = self
            .thread_panel()
            .and(self.thread_cursor().thread())
            .and_then(|shown| here.iter().position(|(_, id)| id == shown));
        let next_here = match (shown, forward) {
            (Some(at), true) => here.get(at + 1),
            (Some(at), false) => at.checked_sub(1).and_then(|at| here.get(at)),
            (None, true) => {
                let line = self.view().cursor_source_line().unwrap_or(0);
                here.iter().find(|(start, _)| *start > line)
            }
            (None, false) => {
                let line = self.view().cursor_source_line().unwrap_or(0);
                here.iter().rev().find(|(start, _)| *start < line)
            }
        };
        if let Some((_, id)) = next_here {
            let id = id.clone();
            self.land_on(&id);
            return;
        }
        // Then the other files, in path order, wrapping.
        let current = self.current.map(|i| self.docs[i].relative.clone());
        let paths: BTreeSet<PathBuf> = self
            .waiting_threads()
            .map(|thread| thread.path().to_path_buf())
            .collect();
        let target = if forward {
            paths
                .iter()
                .find(|path| current.as_deref().is_none_or(|cur| path.as_path() > cur))
                .or_else(|| paths.iter().next())
        } else {
            paths
                .iter()
                .rev()
                .find(|path| current.as_deref().is_none_or(|cur| path.as_path() < cur))
                .or_else(|| paths.iter().next_back())
        };
        let Some(path) = target.cloned() else {
            self.notice("nothing waiting on you");
            return;
        };
        if current.as_deref() != Some(path.as_path()) {
            if !self.workspace.root().join(&path).is_file() {
                self.notice(format!("{} is deleted", path.display()));
                return;
            }
            self.close_popup();
            self.open(&path);
        }
        let here = self.waiting_marks_here();
        let landing = if forward { here.first() } else { here.last() };
        let Some((_, id)) = landing.cloned() else {
            // The store and the marks disagree only while a file is
            // out of scope or unreadable; say so rather than jump blind.
            self.notice("nothing waiting on you");
            return;
        };
        self.land_on(&id);
        self.notice(if forward {
            "wrapped to first waiting thread"
        } else {
            "wrapped to last waiting thread"
        });
    }

    /// `(first line, id)` of the current document's waiting threads, in
    /// line order.
    fn waiting_marks_here(&self) -> Vec<(usize, ThreadId)> {
        let mut marks: Vec<(usize, ThreadId)> = self
            .marks()
            .iter()
            .filter(|mark| mark.kind() == ThreadState::Waiting)
            .map(|mark| (mark.range().start(), mark.id().clone()))
            .collect();
        marks.sort();
        marks
    }

    fn land_on(&mut self, id: &ThreadId) {
        self.show_thread(id.clone());
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::annotations::{Author, Draft, LineRange, Reply, Store};
    use fathomable_core::workspace::Workspace;

    use crate::app::{App, Focus, Options};
    use fathomable_testing::TempDir;

    const README: &str = "# Readme\n\nalpha\nbeta\ngamma\n\n- one\n- two\n";
    const NOTES: &str = "notes\n\nfirst\nsecond\nthird\n";

    fn fixture(name: &str) -> std::io::Result<TempDir> {
        let dir = TempDir::new(&format!("waiting-{name}"))?;
        fs::create_dir_all(dir.0.join("ws"))?;
        fs::write(dir.0.join("ws/README.md"), README)?;
        fs::write(dir.0.join("ws/notes.md"), NOTES)?;
        Ok(dir)
    }

    fn store_path(dir: &TempDir) -> PathBuf {
        dir.0.join("state/threads.jsonl")
    }

    fn app(dir: &TempDir) -> anyhow::Result<App> {
        let workspace = Workspace::discover(dir.0.join("ws"))?;
        let store = Store::open(store_path(dir))?;
        let options = Options {
            store: Some(store),
            ..Options::for_test(dir.0.join("ws"))
        };
        let mut app = App::new(workspace, 100, 30, options);
        app.open(Path::new("README.md"));
        Ok(app)
    }

    /// A second writer, as a headless `--mcp` reply would be: a thread
    /// by the user on `path` at `line`, answered by an agent when
    /// `answered`.
    fn agent_thread(
        store: &mut Store,
        path: &str,
        text: &str,
        line: usize,
        answered: bool,
    ) -> anyhow::Result<()> {
        let id = store.annotate(
            Draft::new(Path::new(path), LineRange::new(line, line), "why?"),
            text,
            10,
        )?;
        if answered {
            store.reply(&id, Reply::new(Author::agent("claude"), 11, "because"))?;
        }
        Ok(())
    }

    /// The first line of the thread whose pane is open.
    fn open_line(app: &App) -> Option<usize> {
        app.thread_panel()?;
        let cursor = app.thread_cursor();
        Some(app.thread(cursor.thread()?)?.range().start())
    }

    /// `l` / `h` walk the file's threads and `L` / `H` the workspace's,
    /// one cursor behind both (ADR 0046).
    #[test]
    fn lowercase_walks_the_file_and_uppercase_the_workspace() -> anyhow::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::app::input::keys;

        let dir = fixture("nav")?;
        let mut app = app(&dir)?;
        let press = |app: &mut App, code| {
            keys::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
        };
        let mut other = Store::open(store_path(&dir))?;
        agent_thread(&mut other, "README.md", README, 3, true)?;
        agent_thread(&mut other, "README.md", README, 5, true)?;
        agent_thread(&mut other, "notes.md", NOTES, 4, true)?;
        app.reload_store();

        app.waiting_next();
        assert_eq!(open_line(&app), Some(3));
        assert_eq!(app.thread_position(), Some((1, 2)));
        assert_eq!(app.thread_position_across(), Some((1, 3)));
        // In the file, `l` wraps within README.
        press(&mut app, KeyCode::Char('l'));
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(open_line(&app), Some(3));
        assert_eq!(app.message(), Some("wrapped to first thread"));

        // Across the workspace, `L` crosses into notes.md and wraps back.
        press(&mut app, KeyCode::Char('L'));
        press(&mut app, KeyCode::Char('L'));
        assert_eq!(app.current_path(), Path::new("notes.md"));
        assert_eq!(open_line(&app), Some(4));
        assert_eq!(app.thread_position(), Some((1, 1)));
        assert_eq!(app.thread_position_across(), Some((3, 3)));
        press(&mut app, KeyCode::Char('L'));
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.thread_position_across(), Some((1, 3)));
        press(&mut app, KeyCode::Char('H'));
        assert_eq!(app.current_path(), Path::new("notes.md"));

        // `h` on the file's only thread hops to the threads pane.
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.focus(), Focus::ThreadsPane);
        assert_eq!(app.current_path(), Path::new("notes.md"));

        // From the text with the pane closed, `]C` and `[C` step from the
        // cursor line and open the other file without opening the pane.
        // README's lines 3-5 render as one paragraph row, so the cursor
        // thread, not the cursor line, says which thread was reached.
        let cursor_line = |app: &App| {
            let cursor = app.thread_cursor();
            app.thread(cursor.thread()?).map(|t| t.range().start())
        };
        app.close_thread();
        app.open(Path::new("README.md"));
        app.view_mut().goto_source_line(1);
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('C'));
        assert_eq!(cursor_line(&app), Some(3));
        assert!(app.thread_panel().is_none(), "`]C` does not open the pane");
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('C'));
        assert_eq!(
            cursor_line(&app),
            Some(5),
            "the cursor tells 3 from 5 on one row"
        );
        press(&mut app, KeyCode::Char(']'));
        press(&mut app, KeyCode::Char('C'));
        assert_eq!(app.current_path(), Path::new("notes.md"));
        assert_eq!(cursor_line(&app), Some(4));
        press(&mut app, KeyCode::Char('['));
        press(&mut app, KeyCode::Char('C'));
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(cursor_line(&app), Some(5));
        // Moving in the text hands the cursor back to the text.
        app.view_mut().goto_top();
        assert_eq!(
            cursor_line(&app),
            Some(3),
            "the cursor rides the text again"
        );
        Ok(())
    }

    #[test]
    fn a_reply_landing_toasts_and_the_keys_walk_waiting_threads() -> anyhow::Result<()> {
        let dir = fixture("walk")?;
        let mut app = app(&dir)?;
        assert_eq!(app.waiting_total(), 0);
        app.waiting_next();
        assert_eq!(app.message(), Some("nothing waiting on you"));

        // Another writer answers two threads and leaves one unanswered.
        let mut other = Store::open(store_path(&dir))?;
        agent_thread(&mut other, "README.md", README, 3, false)?;
        agent_thread(&mut other, "README.md", README, 5, true)?;
        agent_thread(&mut other, "notes.md", NOTES, 4, true)?;
        app.reload_store();
        assert_eq!(
            app.toasts().last().map(crate::app::Toast::text),
            Some("2 replies")
        );
        assert_eq!((app.waiting_count(), app.waiting_total()), (1, 2));
        assert!(app.path_waits(Path::new("notes.md")));
        assert!(!app.path_waits(Path::new("other.md")));

        // ]r lands on README:5 and opens the pane; again crosses into
        // notes.md; a third wraps back with a notice.
        app.waiting_next();
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(open_line(&app), Some(5));
        assert_eq!(app.focus(), Focus::Thread);
        app.waiting_next();
        assert_eq!(app.current_path(), Path::new("notes.md"));
        assert_eq!(open_line(&app), Some(4));
        app.waiting_next();
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.message(), Some("wrapped to first waiting thread"));
        // [r goes back the other way.
        app.waiting_prev();
        assert_eq!(app.current_path(), Path::new("notes.md"));

        // The user's reply ends the wait; a reload that changes nothing
        // raises no toast.
        let toasts = app.toasts().len();
        app.thread_reply();
        app.compose_insert("thanks");
        app.compose_submit();
        assert_eq!(app.waiting_count(), 0);
        app.reload_store();
        assert_eq!(app.toasts().len(), toasts);
        Ok(())
    }
}
