// @okf-doc: /decisions/0049-inline-threads-and-the-rail.md
//! Inline stubs (ADR 0049): a thread shows under the last row of its
//! lines as a block of one or two rows, the newest messages first line
//! each, so a file reads with its conversation in place.
//!
//! Stubs are not lines. The view inserts their rows after the row they
//! hang under, the way a detached thread's row is inserted (ADR 0039),
//! and every motion steps over them; only the mouse lands on one. Which
//! threads produce rows, under which row, in what order, and with which
//! messages is decided here from the marks; the view keeps only the
//! anchors and the row counts, and the drawing asks back for the words.

use fathomable_core::annotations::ThreadId;
use fathomable_core::config::ThreadsConfig;
use fathomable_core::layout::RowAnchor;

use crate::app::App;
use crate::app::threads::{Mark, ThreadState};

/// Messages a collapsed stub shows: the newest two.
const STUB_MESSAGES: usize = 2;

/// Whether stubs are drawn at all, and whether resolved threads get one
/// (`Space c c`, `Space c x`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StubState {
    pub shown: bool,
    pub resolved: bool,
}

impl StubState {
    pub(crate) fn from_config(config: &ThreadsConfig) -> Self {
        Self {
            shown: config.stubs,
            resolved: config.stubs_resolved,
        }
    }
}

/// One thread's stub: where it hangs and which messages it shows,
/// oldest first (zero is the comment).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stub {
    id: ThreadId,
    anchor: RowAnchor,
    messages: Vec<usize>,
}

impl Stub {
    pub fn id(&self) -> &ThreadId {
        &self.id
    }

    /// The messages shown, as indices into the thread: the comment is 0.
    #[cfg(test)]
    pub fn messages(&self) -> &[usize] {
        &self.messages
    }

    /// The row the stub hangs under and how many rows it takes, for the
    /// view.
    pub fn block(&self) -> (RowAnchor, usize) {
        (self.anchor, self.messages.len())
    }
}

impl App {
    /// Whether stubs are drawn (`Space c c`).
    #[cfg(test)]
    pub fn stubs_shown(&self) -> bool {
        self.stubs.shown
    }

    /// Whether resolved threads get a stub (`Space c x`).
    #[cfg(test)]
    pub fn stubs_resolved(&self) -> bool {
        self.stubs.resolved
    }

    /// The current document's stubs in row order: threads by start line,
    /// then end line, then id, so stacked stubs come one thread after
    /// another and never interleave.
    pub fn stubs(&self) -> Vec<Stub> {
        if !self.stubs.shown {
            return Vec::new();
        }
        let mut marks: Vec<&Mark> = self
            .marks()
            .iter()
            .filter(|mark| {
                self.stubs.resolved
                    || matches!(mark.kind(), ThreadState::Open | ThreadState::Waiting)
            })
            .collect();
        marks.sort_by_key(|mark| (mark.range().start(), mark.range().end(), mark.id().clone()));
        marks
            .into_iter()
            .filter_map(|mark| {
                let thread = self.thread(mark.id())?;
                let anchor = if mark.is_detached() {
                    RowAnchor::Detached(self.detached_anchor(mark))
                } else {
                    RowAnchor::Line(mark.range().end())
                };
                let count = thread.replies().len() + 1;
                let messages = (count.saturating_sub(STUB_MESSAGES)..count).collect();
                Some(Stub {
                    id: mark.id().clone(),
                    anchor,
                    messages,
                })
            })
            .collect()
    }

    /// Lay the current document out again with its stub rows in place;
    /// called whenever the marks, the messages, or the toggles change.
    pub(crate) fn place_stub_rows(&mut self) {
        let blocks: Vec<(RowAnchor, usize)> = self.stubs().iter().map(Stub::block).collect();
        self.view_mut().set_stub_blocks(blocks);
    }

    /// The thread whose stub row `row` belongs to, with the message the
    /// row shows and whether it is the block's last row.
    pub fn stub_on_row(&self, row: usize) -> Option<(Stub, usize, bool)> {
        let (block, index) = self.view().stub_slot_of_row(row)?;
        let stub = self.stubs().into_iter().nth(block)?;
        let message = *stub.messages.get(index)?;
        let last = index + 1 == stub.messages.len();
        Some((stub, message, last))
    }

    /// `Space c c`: draw stubs, or not, for the session.
    pub fn toggle_stubs(&mut self) {
        self.stubs.shown = !self.stubs.shown;
        self.place_stub_rows();
        self.push_toast(if self.stubs.shown {
            "stubs shown".to_owned()
        } else {
            "stubs hidden".to_owned()
        });
    }

    /// `Space c x`: give resolved threads a stub too, or not.
    pub fn toggle_resolved_stubs(&mut self) {
        self.stubs.resolved = !self.stubs.resolved;
        self.place_stub_rows();
        self.push_toast(if self.stubs.resolved {
            "resolved stubs shown".to_owned()
        } else {
            "resolved stubs hidden".to_owned()
        });
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use fathomable_core::annotations::Store;
    use fathomable_core::workspace::Workspace;
    use fathomable_testing::TempDir;

    use crate::app::input::keys;
    use crate::app::{App, Options};

    fn fixture(name: &str) -> std::io::Result<TempDir> {
        let dir = TempDir::new(&format!("stubs-{name}"))?;
        fs::create_dir_all(dir.0.join("ws"))?;
        fs::write(
            dir.0.join("ws/README.md"),
            "# Readme\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
        )?;
        Ok(dir)
    }

    fn app(dir: &TempDir) -> anyhow::Result<App> {
        let workspace = Workspace::discover(dir.0.join("ws"))?;
        let store = Store::open(dir.0.join("state/threads.jsonl"))?;
        let options = Options {
            store: Some(store),
            ..Options::for_test(dir.0.join("ws"))
        };
        let mut app = App::new(workspace, 100, 30, options);
        app.open(Path::new("README.md"));
        app.view_mut().toggle_source_view();
        Ok(app)
    }

    fn annotate(app: &mut App, from: usize, to: usize, text: &str) {
        app.view_mut().goto_source_line(from);
        if to > from {
            app.view_mut().select_lines();
            app.view_mut().move_down(to - from);
        }
        app.start_new_comment();
        app.compose_insert(text);
        app.compose_submit();
        app.close_thread();
    }

    fn press(app: &mut App, keys: &str) {
        for ch in keys.chars() {
            keys::handle_key(app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
    }

    fn screen(app: &App) -> anyhow::Result<Vec<String>> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::draw::Theme::from_core(&core);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
        terminal.draw(|frame| crate::app::draw::draw(frame, app, &theme))?;
        let buffer = terminal.backend().buffer().clone();
        Ok((0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect())
    }

    /// Stub rows sit under the last row of their thread, carry no line
    /// number, and stacked threads come one after another in line order;
    /// the cursor steps over them and the hint marks only the cursor's.
    #[test]
    fn stubs_hang_under_their_lines_and_the_cursor_skips_them() -> anyhow::Result<()> {
        let dir = fixture("rows")?;
        let mut app = app(&dir)?;
        annotate(&mut app, 3, 5, "outer thread");
        annotate(&mut app, 5, 5, "inner point");
        app.thread_reply();
        app.compose_insert("agent-free reply");
        app.compose_submit();
        app.close_thread();
        annotate(&mut app, 7, 7, "seven");
        app.view_mut().goto_source_line(1);

        // Row model: 8 source rows, plus 1 + 2 rows under L5 and 1 under L7.
        let rows = app.view().layout().lines().len();
        assert_eq!(rows, 12);
        assert_eq!(app.view().source_line_of_row(4), Some(5));
        assert!(
            app.view().stub_slot_of_row(5).is_some(),
            "outer's stub under L5"
        );
        assert!(
            app.view().stub_slot_of_row(6).is_some(),
            "inner's first row"
        );
        assert!(
            app.view().stub_slot_of_row(7).is_some(),
            "inner's reply row"
        );
        assert_eq!(app.view().source_line_of_row(8), Some(6));
        let (stub, message, last) = app
            .stub_on_row(5)
            .ok_or_else(|| anyhow::anyhow!("a stub under L5"))?;
        assert_eq!(stub.messages(), [0]);
        assert_eq!((message, last), (0, true));
        let (stub, message, last) = app
            .stub_on_row(6)
            .ok_or_else(|| anyhow::anyhow!("a stub under L5"))?;
        assert_eq!(stub.messages(), [0, 1]);
        assert_eq!((message, last), (0, false));
        assert_eq!(app.stub_on_row(7).map(|(_, m, l)| (m, l)), Some((1, true)));

        // The screen: the stub rows show the author, age, and text, no
        // number; the text of the thread under the cursor and the hint.
        app.view_mut().goto_source_line(5);
        let shown = screen(&app)?;
        assert!(shown[4].contains("gamma"), "{:?}", shown[4]);
        assert!(
            shown[5].contains("user") && shown[5].contains("outer thread"),
            "{:?}",
            shown[5]
        );
        assert!(
            !shown[5].contains(" 5 ") && !shown[5].contains(" 6 "),
            "no line number: {:?}",
            shown[5]
        );
        assert!(shown[6].contains("inner point"), "{:?}", shown[6]);
        assert!(shown[7].contains("agent-free reply"), "{:?}", shown[7]);
        assert_eq!(shown[8].trim(), "6", "L6 follows: {:?}", shown[8]);
        // The cursor is on L5, which starts the inner thread: its stub
        // carries the hint on its last row, the outer's does not.
        assert!(shown[7].ends_with("(c expand)"), "{:?}", shown[7]);
        assert!(!shown[5].contains("(c expand)"), "{:?}", shown[5]);
        assert!(!shown[6].contains("(c expand)"), "{:?}", shown[6]);
        // On L4 only the outer thread covers the cursor.
        app.view_mut().goto_source_line(4);
        let shown = screen(&app)?;
        assert!(shown[5].ends_with("(c expand)"), "{:?}", shown[5]);
        assert!(!shown[7].ends_with("(c expand)"), "{:?}", shown[7]);

        // `j` from L5 lands on L6, past three stub rows; `k` comes back.
        app.view_mut().goto_source_line(5);
        press(&mut app, "j");
        assert_eq!(app.view().cursor_source_line(), Some(6));
        assert_eq!(app.view().cursor().row, 8);
        press(&mut app, "k");
        assert_eq!(app.view().cursor().row, 4);
        press(&mut app, "G");
        assert_eq!(
            app.view().source_line_of_row(app.view().cursor().row),
            Some(8)
        );
        press(&mut app, "gg");
        assert_eq!(app.view().cursor().row, 0);
        // `x` selects lines, never a stub.
        app.view_mut().goto_source_line(5);
        press(&mut app, "xx");
        assert_eq!(
            app.view().selected_lines().map(|r| (r.start(), r.end())),
            Some((5, 6))
        );
        press(&mut app, "\u{1b}");
        // A click on a stub row lands on the row it hangs under.
        crate::app::input::mouse::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 60,
                row: 7,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert_eq!(app.view().cursor().row, 4);
        Ok(())
    }

    /// `Space c c` hides and shows every stub; resolved threads have none
    /// until `Space c x`.
    #[test]
    fn the_toggles_hide_stubs_and_resolved_ones() -> anyhow::Result<()> {
        let dir = fixture("toggles")?;
        let mut app = app(&dir)?;
        annotate(&mut app, 3, 3, "three");
        annotate(&mut app, 7, 7, "seven");
        assert_eq!(app.view().layout().lines().len(), 10);
        app.view_mut().goto_source_line(3);
        press(&mut app, " co");
        assert_eq!(app.view().layout().lines().len(), 9, "resolved: no stub");
        assert!(app.stubs().iter().all(|stub| stub.messages().len() == 1));
        press(&mut app, " cx");
        assert_eq!(app.view().layout().lines().len(), 10);
        assert!(app.stubs_resolved());
        press(&mut app, " cx");
        assert_eq!(app.view().layout().lines().len(), 9);
        press(&mut app, " cc");
        assert!(!app.stubs_shown());
        assert_eq!(app.view().layout().lines().len(), 8);
        assert!(app.toasts().iter().any(|t| t.text == "stubs hidden"));
        assert!(app.note_on_row(2).is_some(), "the gutter mark stays");
        press(&mut app, " cc");
        assert_eq!(app.view().layout().lines().len(), 9);
        Ok(())
    }

    /// A detached thread's stub hangs under its own blank row.
    #[test]
    fn a_detached_thread_keeps_its_stub_under_its_row() -> anyhow::Result<()> {
        let dir = fixture("detached")?;
        let mut app = app(&dir)?;
        annotate(&mut app, 4, 4, "on beta");
        fs::write(
            dir.0.join("ws/README.md"),
            "# Readme\n\nalpha\ngamma\n\n- one\n- two\n",
        )?;
        app.on_changes(vec![dir.0.join("ws/README.md")]);
        assert!(app.marks()[0].is_detached());
        let detached = (0..app.view().layout().lines().len())
            .find(|&row| app.view().detached_anchor_of_row(row).is_some())
            .ok_or_else(|| anyhow::anyhow!("a detached row"))?;
        assert!(app.view().stub_slot_of_row(detached + 1).is_some());
        assert_eq!(app.view().source_line_of_row(detached + 2), Some(4));
        Ok(())
    }
}
