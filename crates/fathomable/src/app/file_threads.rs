// @okf-doc: /decisions/0027-revisiting-threads.md
//! The file-threads pane (ADR 0027): the current document's threads, open
//! and resolved, in line order along the bottom of the tree column.
//!
//! The pane keeps no selection of its own. The highlighted row is the
//! thread cursor's (ADR 0046): the thread open in the thread pane, else
//! the thread under the view cursor, derived on every draw and key from
//! the document's marks, so the pane can never disagree with the text or
//! go stale on a reload. Its only state is the height a drag gave it.
//!
//! While the pane has focus the thread pane shows the highlight and
//! follows `j`/`k` and clicks without taking the keys (ADR 0034).

use fathomable_core::annotations::{LineRange, ThreadId};

use super::mark_words::Words;
use super::threads::MarkKind;
use super::{App, Focus};

/// Rows the pane needs before its entries: the rule and the header.
const CHROME_ROWS: usize = 2;
/// The fewest rows the pane is drawn with: rule, header, one entry.
const MIN_ROWS: usize = 3;

/// One entry of the pane, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    range: LineRange,
    kind: MarkKind,
    words: Words,
    /// The first line of the comment.
    summary: String,
    replies: usize,
    /// When the thread last changed.
    updated: u64,
}

impl FileRow {
    pub fn range(&self) -> LineRange {
        self.range
    }

    pub fn kind(&self) -> MarkKind {
        self.kind
    }

    /// Placement and state words (ADR 0032).
    pub fn words(&self) -> Words {
        self.words
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn replies(&self) -> usize {
        self.replies
    }

    pub fn updated(&self) -> u64 {
        self.updated
    }
}

impl App {
    /// The pane's entries in line order; empty when there is nothing to
    /// show.
    pub fn file_thread_rows(&self) -> Vec<FileRow> {
        self.file_threads()
            .into_iter()
            .filter_map(|id| {
                let mark = self.marks().iter().find(|mark| *mark.id() == id)?;
                let thread = self.thread(&id)?;
                Some(FileRow {
                    range: mark.range(),
                    kind: mark.kind(),
                    words: Words::of(Some(mark.placement()), thread),
                    summary: thread.comment().lines().next().unwrap_or("").to_owned(),
                    replies: thread.replies().len(),
                    updated: thread.updated(),
                })
            })
            .collect()
    }

    /// Rows the pane takes at the bottom of the tree column, 0 when the
    /// tree is hidden or the document has no threads: its entries plus
    /// the rule and header, capped at a third of the column, unless its
    /// rule was dragged.
    pub fn file_thread_pane_rows(&self) -> usize {
        if self.tree().is_none() || self.marks().is_empty() {
            return 0;
        }
        let rows = self.pane_rows();
        let tallest = rows.saturating_sub(CHROME_ROWS);
        let least = MIN_ROWS.min(tallest);
        match self.file_rows {
            Some(dragged) => dragged.clamp(least, tallest),
            None => (self.marks().len() + CHROME_ROWS)
                .min((rows / 3).max(MIN_ROWS))
                .clamp(least, tallest),
        }
    }

    /// Rows left to the tree once the pane is taken.
    pub fn sidebar_rows(&self) -> usize {
        self.pane_rows()
            .saturating_sub(self.file_thread_pane_rows())
            .max(1)
    }

    /// The highlighted entry: the thread cursor's thread among the file's
    /// (ADR 0046), `None` when the cursor is in another file.
    pub fn file_thread_selected(&self) -> Option<usize> {
        let order = self.file_threads();
        let cursor = self.thread_cursor();
        let id = cursor.thread()?;
        order.iter().position(|other| other == id)
    }

    /// The first entry drawn, chosen so the highlighted one is on screen.
    pub fn file_thread_scroll(&self) -> usize {
        let body = self
            .file_thread_pane_rows()
            .saturating_sub(CHROME_ROWS)
            .max(1);
        self.file_thread_selected()
            .unwrap_or(0)
            .saturating_sub(body - 1)
    }

    /// `Space t`: focus the pane, showing the tree first when it is
    /// hidden, and open the thread pane on the highlight (ADR 0034); on
    /// the focused pane, close the thread pane it drives and return the
    /// keys to the text.
    pub fn toggle_file_threads(&mut self) {
        if self.focus == Focus::FileThreads {
            self.close_thread();
            self.focus = Focus::View;
        } else {
            self.focus_file_threads();
        }
    }

    /// Focus the pane, showing the tree first when it is hidden, and open
    /// the thread pane on the highlight.
    pub fn focus_file_threads(&mut self) {
        if self.marks().is_empty() {
            self.notice("no threads in this file");
            return;
        }
        if self.tree().is_none() {
            self.show_sidebar();
        }
        if self.tree().is_some() {
            self.focus = Focus::FileThreads;
            self.file_thread_show();
        }
    }

    /// Esc in the pane: the keys go back to the text; the thread pane it
    /// drives stays (ADR 0010, amended 2026-09-03), `Space t` closes it.
    pub fn leave_file_threads(&mut self) {
        if self.focus == Focus::FileThreads {
            self.focus = Focus::View;
        }
    }

    /// `j` / `k` and the wheel: the next or previous entry, wrapping; the
    /// cursor and the thread pane follow, the pane opening if it was not.
    pub fn file_thread_move(&mut self, delta: isize) {
        if self.marks().is_empty() {
            self.notice("no threads in this file");
            return;
        }
        self.thread_step_in_file(delta);
        if self.thread.is_none() {
            self.file_thread_show();
        }
    }

    /// Show the highlighted thread in the thread pane, keys staying here.
    fn file_thread_show(&mut self) {
        if let Some(id) = self.file_thread_id() {
            self.open_thread_behind(id);
        }
    }

    /// A click on the pane's rule row or header: the keys come here and
    /// the thread pane shows the highlight.
    pub fn file_thread_focus(&mut self) {
        self.focus_pane(Focus::FileThreads);
        if self.focus == Focus::FileThreads {
            self.file_thread_show();
        }
    }

    fn file_thread_id(&self) -> Option<ThreadId> {
        self.thread_cursor().thread().cloned()
    }

    /// A click on entry row `row` (counted from the first drawn entry):
    /// the thread pane shows that thread, the cursor on it, and the keys
    /// stay with this pane; a click past the entries acts as one on the
    /// header.
    pub fn file_thread_click(&mut self, row: usize) {
        let index = self.file_thread_scroll() + row;
        self.focus_pane(Focus::FileThreads);
        match self.file_threads().into_iter().nth(index) {
            Some(id) => {
                self.goto_thread(&id);
                self.open_thread_behind(id);
            }
            None => self.file_thread_focus(),
        }
    }

    /// The pane's rule was dragged to screen row `row`.
    pub(super) fn drag_file_threads_to(&mut self, row: usize) {
        self.file_rows = Some(self.pane_rows().saturating_sub(row));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use fathomable_core::annotations::Store;
    use fathomable_core::workspace::Workspace;

    use crate::app::threads::{ComposeTarget, MarkKind};
    use crate::app::{App, Border, Focus, Options, Popup};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> std::io::Result<Self> {
            let dir = std::env::temp_dir().join(format!(
                "fathomable-file-threads-{name}-{}",
                std::process::id()
            ));
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
        for ch in text.chars() {
            app.compose_insert(&ch.to_string());
        }
        app.compose_submit();
    }

    #[test]
    fn the_pane_follows_the_cursor_and_acts_on_the_highlight() -> anyhow::Result<()> {
        let dir = TempDir::new("follow")?;
        let mut app = dir.app()?;
        app.show_sidebar();
        assert_eq!(app.file_thread_pane_rows(), 0, "no threads, no pane");
        app.focus_file_threads();
        assert_eq!(app.message(), Some("no threads in this file"));
        annotate(&mut app, 7, "seven");
        annotate(&mut app, 3, "three");
        annotate(&mut app, 3, "three again");
        let rows = app.file_thread_rows();
        assert_eq!(
            rows.iter()
                .map(|row| (row.range().start(), row.summary()))
                .collect::<Vec<_>>(),
            [(3, "three"), (3, "three again"), (7, "seven")]
        );
        assert_eq!(app.file_thread_pane_rows(), 5, "rule, header, three rows");

        // The highlight is the cursor's thread, else the nearest above.
        app.view_mut().goto_source_line(3);
        assert_eq!(app.file_thread_selected(), Some(0));
        app.view_mut().goto_source_line(8);
        assert_eq!(app.file_thread_selected(), Some(2));
        app.view_mut().goto_source_line(3);
        assert_eq!(app.file_thread_selected(), Some(0));
        app.view_mut().goto_source_line(5);
        assert_eq!(app.file_thread_selected(), Some(1));
        app.view_mut().goto_source_line(1);
        assert_eq!(app.file_thread_selected(), Some(0));
        app.view_mut().goto_source_line(8);
        assert_eq!(app.file_thread_selected(), Some(2));

        // The drawn pane: rule, header with counts, one row per thread,
        // the cursor's thread highlighted.
        app.view_mut().goto_source_line(7);
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::ui::Theme::from_core(&core);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
        terminal.draw(|frame| crate::app::ui::draw(frame, &app, &theme))?;
        let buffer = terminal.backend().buffer().clone();
        let column: Vec<String> = (0..buffer.area.height)
            .map(|y| {
                (0..u16::try_from(app.sidebar_width()).unwrap_or(0))
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect();
        let top = app.sidebar_rows();
        assert!(column[top].starts_with("───"), "rule: {:?}", column[top]);
        assert!(
            column[top + 1].contains("threads 3/3"),
            "{:?}",
            column[top + 1]
        );
        assert!(
            column[top + 2].contains("L3 ● three"),
            "{:?}",
            column[top + 2]
        );
        assert!(
            column[top + 4].contains("L7 ● seven"),
            "{:?}",
            column[top + 4]
        );
        assert!(column[top + 4].contains("↩0 now"), "{:?}", column[top + 4]);

        // `Space t` focuses and opens the thread pane on the highlight;
        // `j`/`k` step by entry, the cursor and the pane following, and
        // the keys stay with the list (ADR 0034).
        app.focus_file_threads();
        assert_eq!(app.focus(), Focus::FileThreads);
        assert_eq!(app.thread_position(), Some((3, 3)));
        app.file_thread_move(1);
        assert_eq!(app.view().cursor_source_line(), Some(3), "wrapped");
        assert_eq!(app.thread_position(), Some((1, 3)));
        app.file_thread_move(1);
        assert_eq!(
            app.view().cursor_source_line(),
            Some(3),
            "two threads on L3"
        );
        assert_eq!(app.thread_position(), Some((2, 3)));
        app.file_thread_move(1);
        assert_eq!(app.view().cursor_source_line(), Some(7));
        app.file_thread_move(-2);
        assert_eq!(app.view().cursor_source_line(), Some(3));
        assert_eq!(app.thread_position(), Some((1, 3)));
        assert_eq!(app.focus(), Focus::FileThreads);

        // Hiding the tree hides the pane; `Space t` brings both back.
        app.hide_sidebar();
        assert_eq!(app.file_thread_pane_rows(), 0);
        app.focus_file_threads();
        assert!(app.tree().is_some());
        assert_eq!(app.focus(), Focus::FileThreads);
        Ok(())
    }

    /// A step from any surface moves the one cursor, and every surface
    /// highlights the same thread (ADR 0046).
    #[test]
    fn every_surface_shows_the_one_cursor() -> anyhow::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::app::input::keys;

        let dir = TempDir::new("cursor")?;
        let mut app = dir.app()?;
        app.show_sidebar();
        annotate(&mut app, 2, "two");
        annotate(&mut app, 6, "six");
        annotate(&mut app, 7, "seven");
        let press = |app: &mut App, code| {
            keys::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
        };
        let ids = app.file_threads();

        // Nothing open: the cursor rides the text.
        app.close_thread();
        app.view_mut().goto_source_line(6);
        assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));
        assert_eq!(app.file_thread_selected(), Some(1));
        app.view_mut().goto_source_line(1);
        assert_eq!(app.thread_cursor().thread(), Some(&ids[0]));

        // The pane: `l` steps, the file-threads highlight follows.
        app.view_mut().goto_source_line(2);
        app.open_thread_at_cursor();
        assert_eq!(app.focus(), Focus::Thread);
        press(&mut app, KeyCode::Char('l'));
        assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));
        assert_eq!(app.file_thread_selected(), Some(1));
        assert_eq!(app.thread_position(), Some((2, 3)));

        // The file-threads pane: `j` steps the same cursor.
        app.focus_file_threads();
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.thread_cursor().thread(), Some(&ids[2]));
        assert_eq!(app.thread_position(), Some((3, 3)));
        assert_eq!(app.view().cursor_source_line(), Some(7));

        // The list opens on it and `h` steps it back.
        app.open_thread_list();
        assert_eq!(app.thread_cursor().thread(), Some(&ids[2]));
        assert_eq!(app.thread_list_selected_index(), Some(2));
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));

        // Enter carries it into the pane; the text lands on its line.
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.focus(), Focus::Thread);
        assert_eq!(app.thread_position(), Some((2, 3)));
        assert_eq!(app.view().cursor_source_line(), Some(6));

        // Closed again, the cursor stays until the reader moves.
        app.close_thread();
        assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));
        app.view_mut().goto_source_line(7);
        assert_eq!(app.thread_cursor().thread(), Some(&ids[2]));
        Ok(())
    }

    #[test]
    fn the_keys_step_between_the_pane_and_the_list() -> anyhow::Result<()> {
        let dir = TempDir::new("step")?;
        let mut app = dir.app()?;
        app.show_sidebar();
        annotate(&mut app, 7, "seven");
        annotate(&mut app, 3, "three");
        annotate(&mut app, 3, "three again");
        app.view_mut().goto_source_line(3);
        app.focus_file_threads();
        assert_eq!(app.focus(), Focus::FileThreads);

        // `r` replies in place; `x` resolves; `l` steps into the thread
        // pane and Left steps back; Esc closes the pane with the focus.
        app.thread_reply();
        assert!(
            matches!(app.popup(), Some(Popup::Compose(c)) if matches!(c.target(), ComposeTarget::Reply(_)))
        );
        app.compose_insert("ok");
        app.compose_submit();
        assert_eq!(app.focus(), Focus::FileThreads);
        assert!(app.thread_panel().is_some());
        assert_eq!(app.file_thread_rows()[0].replies(), 1);
        app.thread_toggle_resolved();
        assert_eq!(app.file_thread_rows()[0].kind(), MarkKind::Resolved);
        app.focus_thread_pane();
        assert_eq!(app.focus(), Focus::Thread);
        assert_eq!(app.thread_position(), Some((1, 3)));
        app.thread_to_file_threads();
        assert_eq!(app.focus(), Focus::FileThreads);
        app.leave_file_threads();
        assert_eq!(app.focus(), Focus::View);
        assert!(
            app.thread_panel().is_some(),
            "Esc leaves, it does not close"
        );
        app.focus_file_threads();
        app.toggle_file_threads();
        assert_eq!(app.focus(), Focus::View);
        assert!(app.thread_panel().is_none(), "Space t on the pane closes");
        // `h` from the pane shows the tree when it was hidden.
        app.hide_sidebar();
        app.focus_thread_pane();
        app.thread_to_file_threads();
        assert!(app.tree().is_some());
        assert_eq!(app.focus(), Focus::FileThreads);
        Ok(())
    }

    #[test]
    fn the_highlight_prefers_the_thread_starting_under_the_cursor() -> anyhow::Result<()> {
        let dir = TempDir::new("overlap")?;
        let mut app = dir.app()?;
        app.show_sidebar();
        // A long thread over L3-5, then a short one at L4 inside it.
        app.view_mut().goto_source_line(3);
        app.view_mut().select_lines();
        app.view_mut().move_down(2);
        app.start_comment();
        app.compose_insert("long");
        app.compose_submit();
        app.close_thread();
        annotate(&mut app, 4, "short");
        app.close_thread();
        assert_eq!(app.file_thread_rows().len(), 2);
        assert_eq!(app.file_thread_rows()[1].range().start(), 4);

        // Clicking the second entry highlights it, not the long thread
        // that also covers L4.
        app.file_thread_click(1);
        assert_eq!(app.thread_position(), Some((2, 2)));
        assert_eq!(app.file_thread_selected(), Some(1));
        app.close_thread();
        assert_eq!(
            app.file_thread_selected(),
            Some(1),
            "L4 starts the short thread"
        );
        app.view_mut().goto_source_line(3);
        assert_eq!(app.file_thread_selected(), Some(0));
        Ok(())
    }

    #[test]
    fn the_highlight_follows_the_open_pane_when_the_cursor_cannot() -> anyhow::Result<()> {
        let dir = TempDir::new("blank")?;
        let mut app = dir.app()?;
        app.show_sidebar();
        annotate(&mut app, 2, "on the blank line");
        annotate(&mut app, 7, "on the list");
        app.close_thread();
        // Rendered view: L2 is blank and has no row, so jumping to that
        // thread leaves the cursor where it was.
        app.view_mut().toggle_source_view();
        app.view_mut().goto_source_line(7);
        assert_eq!(app.file_thread_selected(), Some(1));
        app.file_thread_click(0);
        assert_eq!(app.thread_position(), Some((1, 2)));
        assert_eq!(app.file_thread_selected(), Some(0));
        app.thread_step_in_file(1);
        assert_eq!(app.file_thread_selected(), Some(1));
        Ok(())
    }

    #[test]
    fn the_mouse_clicks_wheels_and_drags_the_pane() -> anyhow::Result<()> {
        let dir = TempDir::new("mouse")?;
        let mut app = dir.app()?;
        app.show_sidebar();
        annotate(&mut app, 2, "two");
        annotate(&mut app, 6, "six");
        app.view_mut().goto_source_line(1);
        let mouse = |kind, column: usize, row: usize| MouseEvent {
            kind,
            column: u16::try_from(column).unwrap_or(u16::MAX),
            row: u16::try_from(row).unwrap_or(u16::MAX),
            modifiers: KeyModifiers::NONE,
        };
        let down = MouseEventKind::Down(MouseButton::Left);
        let top = app.sidebar_rows();
        assert_eq!(app.pane_rows() - top, 4);

        // A click on an entry shows its thread in the thread pane, cursor
        // on the thread, and the keys stay with this pane (ADR 0034).
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 2, top + 3));
        assert_eq!(app.view().cursor_source_line(), Some(6));
        assert_eq!(app.focus(), Focus::FileThreads);
        assert_eq!(app.thread_position(), Some((2, 2)));
        app.close_thread();
        // A click on the header focuses the pane and shows the highlight.
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 2, top + 1));
        assert_eq!(app.focus(), Focus::FileThreads);
        assert_eq!(app.thread_position(), Some((2, 2)));
        // The wheel steps between threads.
        crate::app::input::mouse::handle_mouse(
            &mut app,
            mouse(MouseEventKind::ScrollUp, 2, top + 1),
        );
        assert_eq!(app.view().cursor_source_line(), Some(2));
        // A click on the tree above leaves the pane.
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 2, 1));
        assert_eq!(app.focus(), Focus::Sidebar);
        // Dragging the rule resizes the pane and shrinks the tree.
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 2, top));
        assert_eq!(app.dragging(), Some(Border::FileThreads));
        crate::app::input::mouse::handle_mouse(
            &mut app,
            mouse(MouseEventKind::Drag(MouseButton::Left), 2, top - 3),
        );
        crate::app::input::mouse::handle_mouse(
            &mut app,
            mouse(MouseEventKind::Up(MouseButton::Left), 2, top - 3),
        );
        assert_eq!(app.file_thread_pane_rows(), 7);
        assert_eq!(app.sidebar_rows(), top - 3);
        Ok(())
    }
}
