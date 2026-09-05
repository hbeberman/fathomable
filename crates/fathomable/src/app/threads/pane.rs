// @okf-doc: /decisions/0027-revisiting-threads.md
//! The threads pane (ADR 0027, reshaped by ADR 0049): the lower pane of
//! the rail, listing this file's threads in line order or the whole
//! workspace's by file and line, resolved ones hidden until asked for.
//!
//! The pane keeps no selection of its own. The highlighted row is the
//! thread cursor's (ADR 0046), derived on every draw and key from the
//! marks and the store, so the pane can never disagree with the text or
//! go stale on a reload. Its state is whether it is shown, its scope,
//! and the split a drag gave it; the resolved flag is the review's,
//! shared with the review list.

use std::path::PathBuf;

use fathomable_core::annotations::{LineRange, ThreadId};
use fathomable_core::config::RailConfig;

use crate::app::threads::words::Words;
use crate::app::threads::{Mark, ThreadState};
use crate::app::{App, Focus};

/// The rail's state (ADR 0049): which of its panes are shown, what the
/// threads pane lists, the split a drag set, and the configured sizes.
#[derive(Debug)]
pub(crate) struct Rail {
    /// The tree pane is shown.
    pub(crate) tree: bool,
    /// The threads pane is shown.
    pub(crate) threads: bool,
    pub(crate) scope: PaneScope,
    /// Rows a drag gave the threads pane, over `config.split`.
    pub(crate) split: Option<usize>,
    pub(crate) config: RailConfig,
}

impl Rail {
    pub(crate) fn new(config: RailConfig) -> Self {
        Self {
            tree: false,
            threads: false,
            scope: PaneScope::default(),
            split: None,
            config,
        }
    }
}

/// Rows the pane needs before its entries: the rule and the header.
const CHROME_ROWS: usize = 2;
/// The fewest rows the pane is drawn with: rule, header, one entry.
const MIN_ROWS: usize = 3;
/// Rows the tree keeps above the pane when both are shown.
const TREE_MIN_ROWS: usize = 2;

/// Which threads the pane lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PaneScope {
    /// The current document's threads in line order.
    #[default]
    File,
    /// Every thread on the work, files by path, threads by line.
    Workspace,
}

impl PaneScope {
    /// The word the header shows.
    #[must_use]
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Workspace => "workspace",
        }
    }
}

/// One entry of the pane, ready to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneRow {
    path: PathBuf,
    range: LineRange,
    kind: ThreadState,
    words: Words,
    /// The first line of the newest message.
    summary: String,
    replies: usize,
    /// When the thread last changed.
    updated: u64,
}

impl PaneRow {
    #[cfg(test)]
    pub(crate) fn range(&self) -> LineRange {
        self.range
    }

    pub(crate) fn kind(&self) -> ThreadState {
        self.kind
    }

    /// Placement and state words (ADR 0032).
    pub(crate) fn words(&self) -> Words {
        self.words
    }

    pub(crate) fn summary(&self) -> &str {
        &self.summary
    }

    pub(crate) fn replies(&self) -> usize {
        self.replies
    }

    pub(crate) fn updated(&self) -> u64 {
        self.updated
    }

    /// Where the thread is, as the scope names it: `L9-11` in the file,
    /// `lib.rs:9` across the workspace.
    #[must_use]
    pub(crate) fn place(&self, scope: PaneScope) -> String {
        match scope {
            PaneScope::File => format!("L{}", self.range),
            PaneScope::Workspace => {
                let name = self
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{name}:{}", self.range.start())
            }
        }
    }
}

impl App {
    /// Whether the pane is shown in the rail.
    pub(crate) fn threads_pane_shown(&self) -> bool {
        self.rail.threads
    }

    /// Which threads the pane lists.
    pub(crate) fn rail_scope(&self) -> PaneScope {
        self.rail.scope
    }

    /// The threads the pane lists, in its order, resolved ones only when
    /// the review shows them.
    pub(crate) fn threads_pane_ids(&self) -> Vec<ThreadId> {
        match self.rail.scope {
            // Line order, resolved ones when the review shows them.
            PaneScope::File => {
                let order = self.file_threads();
                if self.review.resolved {
                    return order;
                }
                order
                    .into_iter()
                    .filter(|id| {
                        self.thread(id).is_some_and(|thread| {
                            matches!(
                                self.placement_of(thread).1,
                                ThreadState::Open | ThreadState::Waiting
                            )
                        })
                    })
                    .collect()
            }
            // The review's order (ADR 0049).
            PaneScope::Workspace => self
                .review_entries(false)
                .into_iter()
                .map(|entry| entry.id().clone())
                .collect(),
        }
    }

    /// The pane's entries, ready to draw.
    pub(crate) fn threads_pane_rows(&self) -> Vec<PaneRow> {
        self.threads_pane_ids()
            .into_iter()
            .filter_map(|id| {
                let thread = self.thread(&id)?;
                let (range, kind) = self.placement_of(thread);
                let placement = self
                    .marks()
                    .iter()
                    .find(|mark| *mark.id() == id)
                    .map(Mark::placement);
                let newest = thread
                    .replies()
                    .last()
                    .map_or(thread.comment(), |reply| reply.body());
                Some(PaneRow {
                    path: thread.path().to_path_buf(),
                    range,
                    kind,
                    words: Words::of(placement, thread),
                    summary: newest.lines().next().unwrap_or("").to_owned(),
                    replies: thread.replies().len(),
                    updated: thread.updated(),
                })
            })
            .collect()
    }

    /// Rows the pane takes at the bottom of the rail: 0 when hidden, the
    /// whole column when the tree is hidden, else the split `rail.split`
    /// or a drag set, kept between one entry and the tree's minimum.
    pub(crate) fn threads_pane_height(&self) -> usize {
        if !self.rail.threads {
            return 0;
        }
        let rows = self.pane_rows();
        if self.tree().is_none() {
            return rows;
        }
        let tallest = rows.saturating_sub(TREE_MIN_ROWS);
        let least = MIN_ROWS.min(tallest);
        self.rail
            .split
            .unwrap_or(self.rail.config.split)
            .clamp(least, tallest)
    }

    /// Rows the tree pane has, 0 when it is hidden.
    pub(crate) fn tree_rows(&self) -> usize {
        if self.tree().is_none() {
            return 0;
        }
        self.pane_rows()
            .saturating_sub(self.threads_pane_height())
            .max(1)
    }

    /// The highlighted entry: the thread cursor's thread among the listed
    /// ones (ADR 0046), `None` when it is not listed.
    pub(crate) fn threads_pane_selected(&self) -> Option<usize> {
        let order = self.threads_pane_ids();
        let cursor = self.thread_cursor();
        let id = cursor.thread()?;
        order.iter().position(|other| other == id)
    }

    /// The first entry drawn, chosen so the highlighted one is on screen.
    pub(crate) fn threads_pane_scroll(&self) -> usize {
        let body = self
            .threads_pane_height()
            .saturating_sub(CHROME_ROWS)
            .max(1);
        self.threads_pane_selected()
            .unwrap_or(0)
            .saturating_sub(body - 1)
    }

    /// `Space t`: show and focus the pane, or hand the keys back to the
    /// text when it has them; the pane stays either way.
    pub(crate) fn toggle_threads_pane(&mut self) {
        if self.focus == Focus::ThreadsPane {
            self.focus = Focus::View;
        } else {
            self.focus_threads_pane();
        }
    }

    /// `Space T`: hide the pane, or show it again without taking the
    /// keys; the tree keeps the rail if it is shown.
    pub(crate) fn toggle_threads_pane_shown(&mut self) {
        if !self.rail.threads {
            self.show_threads_pane();
            return;
        }
        self.rail.threads = false;
        if self.focus == Focus::ThreadsPane {
            self.focus = Focus::View;
        }
        self.relayout();
    }

    /// Show the pane without taking the keys, as a workspace start does.
    pub(crate) fn show_threads_pane(&mut self) {
        if !self.rail.threads {
            self.rail.threads = true;
            self.relayout();
        }
    }

    /// Show the pane and give it the keys.
    pub(crate) fn focus_threads_pane(&mut self) {
        self.show_threads_pane();
        self.focus = Focus::ThreadsPane;
    }

    /// Esc in the pane: the keys go back to the text; the pane stays.
    pub(crate) fn leave_threads_pane(&mut self) {
        if self.focus == Focus::ThreadsPane {
            self.focus = Focus::View;
        }
    }

    /// `j` / `k` and the wheel: the next or previous listed thread,
    /// wrapping; the text follows, another file opening in workspace
    /// scope, and the keys stay here.
    pub(crate) fn threads_pane_move(&mut self, delta: isize) {
        let order = self.threads_pane_ids();
        if order.is_empty() {
            self.notice(self.empty_pane_notice());
            return;
        }
        if let Some(id) = self.step_in(&order, delta) {
            self.land_in_pane(id);
        }
    }

    /// `s`: list this file, or the whole workspace.
    pub(crate) fn threads_pane_toggle_scope(&mut self) {
        self.rail.scope = match self.rail.scope {
            PaneScope::File => PaneScope::Workspace,
            PaneScope::Workspace => PaneScope::File,
        };
        self.notice(format!("threads: {}", self.rail.scope.word()));
    }

    /// A click on the pane's rule row or header: the keys come here.
    pub(crate) fn threads_pane_focus(&mut self) {
        self.focus_pane(Focus::ThreadsPane);
    }

    /// A click on entry row `row` (counted from the first drawn entry):
    /// the cursor goes to that thread and the keys stay with this pane;
    /// a click past the entries acts as one on the header.
    pub(crate) fn threads_pane_click(&mut self, row: usize) {
        let index = self.threads_pane_scroll() + row;
        match self.threads_pane_ids().into_iter().nth(index) {
            Some(id) => self.land_in_pane(id),
            None => self.threads_pane_focus(),
        }
    }

    /// Enter: open the file with the cursor's thread expanded and the
    /// keys going to the text (ADR 0049).
    pub(crate) fn threads_pane_open(&mut self) {
        let Some(id) = self.thread_cursor().thread().cloned() else {
            return;
        };
        if self.land_on_thread(id.clone()) {
            let newest = self.newest_message(&id);
            self.goto_message(id, newest);
            self.focus = Focus::View;
        }
    }

    /// The pane's rule was dragged to screen row `row`.
    pub(crate) fn drag_threads_pane_to(&mut self, row: usize) {
        self.rail.split = Some(self.pane_rows().saturating_sub(row));
    }

    /// Land the cursor on `id` from the pane: the file opens if it is
    /// elsewhere and the keys stay with the pane.
    fn land_in_pane(&mut self, id: ThreadId) {
        if self.land_on_thread(id) && self.rail.threads {
            self.focus = Focus::ThreadsPane;
        }
    }

    fn empty_pane_notice(&self) -> &'static str {
        match self.rail.scope {
            PaneScope::File => "no threads in this file",
            PaneScope::Workspace => "no threads in the workspace",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use fathomable_core::annotations::{Author, Reply, Thread};
    use fathomable_testing::TempDir;

    use crate::app::testing::{self, press, source_app};

    use super::PaneScope;
    use crate::app::input::keys;
    use crate::app::threads::{ComposeTarget, ThreadState};
    use crate::app::{App, Border, Focus, Popup};

    fn fixture(name: &str) -> std::io::Result<TempDir> {
        let dir = testing::workspace(&format!("threads-pane-{name}"), testing::README)?;
        fs::create_dir_all(dir.0.join("ws/docs"))?;
        fs::write(dir.0.join("ws/docs/guide.md"), "# Guide\n\nfirst\nsecond\n")?;
        Ok(dir)
    }

    fn annotate(app: &mut App, line: usize, text: &str) {
        app.view_mut().goto_source_line(line);
        app.start_new_comment();
        app.compose_insert(text);
        app.compose_submit();
    }

    /// A user reply to the thread of mark `index`, dated after every
    /// thread's `updated`. That field has one-second resolution, so a
    /// thread written in a later second than another sorts before it in
    /// the review's order and one written in the same second ties; the
    /// dated answer makes the order the test's, not the clock's.
    fn answer_later(app: &mut App, index: usize) -> anyhow::Result<()> {
        let id = app.marks()[index].id().clone();
        let store = app.store_mut().ok_or_else(|| anyhow::anyhow!("no store"))?;
        let later = store
            .threads()
            .iter()
            .map(Thread::updated)
            .max()
            .unwrap_or(0)
            + 1;
        store.reply(&id, Reply::new(Author::User, later, "later"))?;
        Ok(())
    }

    fn rail_column(app: &App) -> anyhow::Result<Vec<String>> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::draw::Theme::from_core(&core);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
        terminal.draw(|frame| crate::app::draw::draw(frame, app, &theme))?;
        let buffer = terminal.backend().buffer().clone();
        Ok((0..buffer.area.height)
            .map(|y| {
                (0..u16::try_from(app.rail_width()).unwrap_or(0))
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect())
    }

    /// The pane lists this file in line order, hides resolved threads
    /// until `x`, and lists the workspace after `s`, where `j` opens the
    /// other file and keeps the keys (ADR 0049).
    #[test]
    fn the_pane_lists_the_file_or_the_workspace_and_hides_resolved() -> anyhow::Result<()> {
        let dir = fixture("scope")?;
        let mut app = source_app(&dir)?;
        app.show_tree();
        app.show_threads_pane();
        annotate(&mut app, 7, "seven");
        annotate(&mut app, 3, "three");
        app.open(Path::new("docs/guide.md"));
        annotate(&mut app, 3, "guide first");
        app.open(Path::new("README.md"));
        app.view_mut().goto_source_line(3);

        let rows = app.threads_pane_rows();
        assert_eq!(
            rows.iter()
                .map(|row| (row.place(PaneScope::File), row.summary()))
                .collect::<Vec<_>>(),
            [("L3".to_owned(), "three"), ("L7".to_owned(), "seven")]
        );
        assert_eq!(app.threads_pane_selected(), Some(0));

        // The drawn pane: rule, header with the scope and count, one row
        // per thread with the glyph, place, and the newest message.
        let column = rail_column(&app)?;
        let top = app.tree_rows();
        assert!(column[top].starts_with("───"), "rule: {:?}", column[top]);
        assert!(
            column[top + 1].contains("threads · file 2"),
            "{:?}",
            column[top + 1]
        );
        assert!(column[top + 1].contains("s x"), "{:?}", column[top + 1]);
        assert!(
            column[top + 2].contains("● L3 three"),
            "{:?}",
            column[top + 2]
        );
        assert!(
            column[top + 3].contains("● L7 seven"),
            "{:?}",
            column[top + 3]
        );
        assert!(column[top + 3].contains(" now"), "{:?}", column[top + 3]);

        // A reply becomes the summary and earns the reply count.
        app.thread_reply();
        app.compose_insert("answered");
        app.compose_submit();
        assert_eq!(app.threads_pane_rows()[0].summary(), "answered");
        assert_eq!(app.threads_pane_rows()[0].replies(), 1);

        // Resolved threads leave the list until `x` shows them.
        app.focus_threads_pane();
        assert_eq!(app.focus(), Focus::ThreadsPane);
        press(&mut app, "o");
        assert_eq!(app.marks()[1].kind(), ThreadState::Resolved, "L3 resolved");
        assert_eq!(app.threads_pane_rows().len(), 1);
        assert_eq!(app.threads_pane_rows()[0].range().start(), 7);
        press(&mut app, "x");
        assert_eq!(app.message(), Some("resolved shown"));
        assert_eq!(app.threads_pane_rows().len(), 2);
        assert_eq!(app.threads_pane_rows()[0].kind(), ThreadState::Resolved);
        press(&mut app, "x");
        assert_eq!(app.threads_pane_rows().len(), 1);

        // `s` lists the workspace in the review's order, newest first,
        // the place saying which file.
        answer_later(&mut app, 0)?;
        press(&mut app, "s");
        assert_eq!(app.rail_scope(), PaneScope::Workspace);
        let rows = app.threads_pane_rows();
        assert_eq!(
            rows.iter()
                .map(|row| row.place(PaneScope::Workspace))
                .collect::<Vec<_>>(),
            ["README.md:7", "guide.md:3"]
        );
        let column = rail_column(&app)?;
        assert!(
            column[app.tree_rows() + 1].contains("threads · workspace 2"),
            "{:?}",
            column[app.tree_rows() + 1]
        );

        // `j` steps across files and keeps the keys in the pane.
        app.view_mut().goto_source_line(7);
        press(&mut app, "j");
        assert_eq!(app.current_path(), Path::new("docs/guide.md"));
        assert_eq!(app.view().cursor_source_line(), Some(3));
        assert_eq!(app.focus(), Focus::ThreadsPane);
        assert_eq!(app.threads_pane_selected(), Some(1));
        press(&mut app, "j");
        assert_eq!(app.current_path(), Path::new("README.md"), "wrapped");
        assert_eq!(app.focus(), Focus::ThreadsPane);

        // Enter opens the thread expanded with the keys in the text; `r`
        // replies in place with the keys staying here.
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.focus(), Focus::View);
        assert!(app.shows_thread());
        app.focus_threads_pane();
        press(&mut app, "r");
        assert!(
            matches!(app.popup(), Some(Popup::Compose(c)) if matches!(c.target(), ComposeTarget::Reply(_)))
        );
        app.compose_insert("ok");
        app.compose_submit();
        assert_eq!(app.focus(), Focus::ThreadsPane);
        Ok(())
    }

    /// The pane shows with the tree hidden and takes the whole rail;
    /// beside the tree its split is fixed whatever the thread count, and
    /// only a drag changes it. `Space e`, `E`, `t`, and `T` show, hide,
    /// and focus each pane on its own.
    #[test]
    fn the_rail_shows_either_pane_and_the_split_is_fixed() -> anyhow::Result<()> {
        let dir = fixture("split")?;
        let mut app = source_app(&dir)?;
        assert_eq!(app.rail_width(), 0);
        assert_eq!(app.threads_pane_height(), 0);

        // `Space t` with nothing shown: the pane alone fills the rail.
        press(&mut app, " t");
        assert_eq!(app.focus(), Focus::ThreadsPane);
        assert!(app.tree().is_none());
        assert_eq!(app.rail_width(), 32);
        assert_eq!(app.threads_pane_height(), app.pane_rows());
        assert_eq!(app.tree_rows(), 0);
        let column = rail_column(&app)?;
        assert!(column[1].contains("threads · file 0"), "{:?}", column[1]);
        assert!(
            column[2].contains("no threads in this file"),
            "{:?}",
            column[2]
        );

        // `Space t` again hands the keys back; the pane stays.
        press(&mut app, " t");
        assert_eq!(app.focus(), Focus::View);
        assert!(app.threads_pane_shown());

        // The tree joins above at the configured split of 8 rows, and a
        // dozen threads do not grow it.
        press(&mut app, " e");
        assert_eq!(app.focus(), Focus::Tree);
        assert_eq!(app.threads_pane_height(), 8);
        assert_eq!(app.tree_rows(), app.pane_rows() - 8);
        for line in 1..=8 {
            annotate(&mut app, line, "many");
        }
        assert_eq!(app.threads_pane_rows().len(), 8);
        assert_eq!(app.threads_pane_height(), 8, "the split is fixed");

        // A drag on the rule changes it for the session.
        let mouse = |kind, row: usize| MouseEvent {
            kind,
            column: 2,
            row: u16::try_from(row).unwrap_or(u16::MAX),
            modifiers: KeyModifiers::NONE,
        };
        let top = app.tree_rows();
        crate::app::input::mouse::handle_mouse(
            &mut app,
            mouse(MouseEventKind::Down(MouseButton::Left), top),
        );
        assert_eq!(app.dragging(), Some(Border::ThreadsPane));
        crate::app::input::mouse::handle_mouse(
            &mut app,
            mouse(MouseEventKind::Drag(MouseButton::Left), top - 4),
        );
        crate::app::input::mouse::handle_mouse(
            &mut app,
            mouse(MouseEventKind::Up(MouseButton::Left), top - 4),
        );
        assert_eq!(app.threads_pane_height(), 12);
        assert_eq!(app.tree_rows(), top - 4);

        // `Space E` hides the tree and leaves the pane; `Space T` hides
        // the pane, and with both gone the rail goes. Both keys show
        // their pane again without taking the keys.
        app.focus_threads_pane();
        press(&mut app, " E");
        assert!(app.tree().is_none());
        assert!(app.threads_pane_shown());
        assert_eq!(
            app.focus(),
            Focus::ThreadsPane,
            "the keys stay with the pane"
        );
        assert_eq!(app.rail_width(), 32);
        press(&mut app, " T");
        assert!(!app.threads_pane_shown());
        assert_eq!(app.focus(), Focus::View);
        assert_eq!(app.rail_width(), 0);
        press(&mut app, " T");
        assert!(app.threads_pane_shown(), "the same key shows it again");
        assert_eq!(app.focus(), Focus::View, "showing does not take the keys");
        press(&mut app, " E");
        assert!(app.tree().is_some(), "the same key shows it again");
        assert_eq!(app.focus(), Focus::View, "showing does not take the keys");
        Ok(())
    }

    /// A step from any surface moves the one cursor, and every surface
    /// highlights the same thread (ADR 0046).
    #[test]
    fn every_surface_shows_the_one_cursor() -> anyhow::Result<()> {
        let dir = fixture("cursor")?;
        let mut app = source_app(&dir)?;
        app.show_tree();
        app.show_threads_pane();
        annotate(&mut app, 2, "two");
        annotate(&mut app, 6, "six");
        annotate(&mut app, 7, "seven");
        let ids = app.file_threads();

        // Nothing open: the cursor rides the text.
        app.view_mut().goto_source_line(6);
        assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));
        assert_eq!(app.threads_pane_selected(), Some(1));
        app.view_mut().goto_source_line(1);
        assert_eq!(app.thread_cursor().thread(), Some(&ids[0]));

        // The text: `]c` steps, the threads pane's highlight follows.
        app.focus_pane(Focus::View);
        app.view_mut().goto_source_line(2);
        app.expand_at_cursor();
        assert_eq!(app.focus(), Focus::View);
        press(&mut app, "]c");
        assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));
        assert_eq!(app.threads_pane_selected(), Some(1));
        assert_eq!(app.thread_position(), Some((2, 3)));

        // The threads pane: `j` steps the same cursor.
        app.focus_threads_pane();
        press(&mut app, "j");
        assert_eq!(app.thread_cursor().thread(), Some(&ids[2]));
        assert_eq!(app.thread_position(), Some((3, 3)));
        assert_eq!(app.view().cursor_source_line(), Some(7));

        // The list opens on it and `h` steps it back.
        app.open_review();
        assert_eq!(app.thread_cursor().thread(), Some(&ids[2]));
        assert_eq!(app.review_selected_index(), Some(2));
        press(&mut app, "h");
        assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));

        // Enter expands it in the text; the cursor lands on its line.
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.focus(), Focus::View);
        assert_eq!(app.thread_position(), Some((2, 3)));
        assert_eq!(app.view().cursor_source_line(), Some(6));

        // The cursor stays until the reader moves.
        assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));
        app.view_mut().goto_source_line(7);
        assert_eq!(app.thread_cursor().thread(), Some(&ids[2]));

        // Esc in the threads pane leaves; the pane stays.
        app.focus_threads_pane();
        app.leave_threads_pane();
        assert_eq!(app.focus(), Focus::View);
        assert!(app.threads_pane_shown(), "Esc leaves, it does not hide");
        Ok(())
    }

    #[test]
    fn the_highlight_prefers_the_thread_starting_under_the_cursor() -> anyhow::Result<()> {
        let dir = fixture("overlap")?;
        let mut app = source_app(&dir)?;
        app.show_tree();
        app.show_threads_pane();
        // A long thread over L3-5, then a short one at L4 inside it.
        app.view_mut().goto_source_line(3);
        app.view_mut().select_lines();
        app.view_mut().move_down(2);
        app.start_comment();
        app.compose_insert("long");
        app.compose_submit();
        annotate(&mut app, 4, "short");
        assert_eq!(app.threads_pane_rows().len(), 2);
        assert_eq!(app.threads_pane_rows()[1].range().start(), 4);

        // Clicking the second entry highlights it, not the long thread
        // that also covers L4.
        app.threads_pane_click(1);
        assert_eq!(app.threads_pane_selected(), Some(1));
        assert_eq!(app.focus(), Focus::ThreadsPane);
        app.view_mut().goto_source_line(3);
        assert_eq!(app.threads_pane_selected(), Some(0));
        Ok(())
    }

    #[test]
    fn the_mouse_clicks_wheels_and_focuses_the_pane() -> anyhow::Result<()> {
        let dir = fixture("mouse")?;
        let mut app = source_app(&dir)?;
        app.show_tree();
        app.show_threads_pane();
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
        let top = app.tree_rows();
        assert_eq!(app.pane_rows() - top, 8);

        // A click on an entry puts the cursor on its thread and the keys
        // with the pane.
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 2, top + 3));
        assert_eq!(app.view().cursor_source_line(), Some(6));
        assert_eq!(app.focus(), Focus::ThreadsPane);
        // A click on the header focuses the pane.
        app.focus_pane(Focus::View);
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 2, top + 1));
        assert_eq!(app.focus(), Focus::ThreadsPane);
        // The wheel steps between threads.
        crate::app::input::mouse::handle_mouse(
            &mut app,
            mouse(MouseEventKind::ScrollUp, 2, top + 1),
        );
        assert_eq!(app.view().cursor_source_line(), Some(2));
        // A click on the tree above leaves the pane.
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 2, 1));
        assert_eq!(app.focus(), Focus::Tree);
        Ok(())
    }
}
