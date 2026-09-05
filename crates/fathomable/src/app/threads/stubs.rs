// @okf-doc: /decisions/0049-inline-threads-and-the-rail.md
//! Inline stubs (ADR 0049): a thread shows under the last row of its
//! lines as a block of one or two rows, the newest messages' first line
//! each, and `c` expands it in place into the whole thread, so a file
//! reads with its conversation where the lines are.
//!
//! Stubs are not lines. The view inserts their rows after the row they
//! hang under, the way a detached thread's row is inserted (ADR 0039),
//! and every motion steps over a collapsed stub; only the mouse lands on
//! one. An expanded thread's message rows are stops: `j`/`k` walk them,
//! and the message under the cursor is the thread cursor's. Which
//! threads produce rows, under which row, in what order, with which
//! messages, and which rows are stops is decided here from the marks;
//! the view keeps only the anchors, counts, and stops, and the drawing
//! asks back for the words.

use fathomable_core::annotations::ThreadId;
use fathomable_core::config::ThreadsConfig;
use fathomable_core::layout::RowAnchor;

use crate::app::App;
use crate::app::draw::message::expanded_rows;
use crate::app::threads::{Mark, ThreadState};
use crate::app::view::StubBlock;

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

/// One thread's stub: where it hangs, which messages it shows (oldest
/// first, zero the comment), and its shape when expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stub {
    id: ThreadId,
    anchor: RowAnchor,
    messages: Vec<usize>,
    expanded: bool,
    /// Rows the block takes.
    rows: usize,
    /// Row indices within the block the cursor may rest on: an expanded
    /// thread's message rows. Empty for a collapsed stub.
    stops: Vec<usize>,
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

    /// Whether the whole thread shows.
    pub fn expanded(&self) -> bool {
        self.expanded
    }

    /// The message row `index` of the block belongs to: for a collapsed
    /// stub the message on that row, for an expanded thread the message
    /// whose header or body it is; `None` on the expanded header row.
    #[must_use]
    pub fn message_of_row(&self, index: usize) -> Option<usize> {
        if !self.expanded {
            return self.messages.get(index).copied();
        }
        let position = self.stops.iter().rposition(|&stop| stop <= index)?;
        self.messages.get(position).copied()
    }

    /// The row within the block that message `message` starts on, when
    /// the thread is expanded.
    #[must_use]
    pub fn row_of_message(&self, message: usize) -> Option<usize> {
        if !self.expanded {
            return None;
        }
        let position = self.messages.iter().position(|&m| m == message)?;
        self.stops.get(position).copied()
    }

    /// The block as the view lays it out.
    pub fn block(&self) -> StubBlock {
        StubBlock {
            anchor: self.anchor,
            rows: self.rows,
            stops: self.stops.clone(),
        }
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

    /// Whether `id` is expanded in place.
    pub fn is_expanded(&self, id: &ThreadId) -> bool {
        self.expanded.contains(id)
    }

    /// The current document's stubs in row order: threads by start line,
    /// then end line, then id, so stacked stubs come one thread after
    /// another and never interleave.
    pub fn stubs(&self) -> Vec<Stub> {
        if !self.stubs.shown {
            return Vec::new();
        }
        let width = self.view().layout().width();
        let mut marks: Vec<&Mark> = self
            .marks()
            .iter()
            .filter(|mark| {
                // A resolved thread has no stub unless asked for, but an
                // expanded one always has its rows.
                self.stubs.resolved
                    || matches!(mark.kind(), ThreadState::Open | ThreadState::Waiting)
                    || self.expanded.contains(mark.id())
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
                let expanded = self.expanded.contains(mark.id());
                let (messages, rows, stops) = if expanded {
                    let (rows, stops) = expanded_rows(thread, width, self.highlighter());
                    // The header row comes first; the stops follow it.
                    (
                        (0..count).collect(),
                        rows + 1,
                        stops.into_iter().map(|stop| stop + 1).collect(),
                    )
                } else {
                    let messages: Vec<usize> =
                        (count.saturating_sub(STUB_MESSAGES)..count).collect();
                    let rows = messages.len();
                    (messages, rows, Vec::new())
                };
                Some(Stub {
                    id: mark.id().clone(),
                    anchor,
                    messages,
                    expanded,
                    rows,
                    stops,
                })
            })
            .collect()
    }

    /// Lay the current document out again with its stub rows in place;
    /// called whenever the marks, the messages, or the toggles change.
    pub(crate) fn place_stub_rows(&mut self) {
        let blocks: Vec<StubBlock> = self.stubs().iter().map(Stub::block).collect();
        self.view_mut().set_stub_blocks(blocks);
    }

    /// The stub whose row `row` is, with the row's index in the block
    /// and whether it is the block's last row.
    pub fn stub_on_row(&self, row: usize) -> Option<(Stub, usize, bool)> {
        let (block, index) = self.view().stub_slot_of_row(row)?;
        let stub = self.stubs().into_iter().nth(block)?;
        let last = index + 1 == stub.rows;
        Some((stub, index, last))
    }

    /// The thread and message an expanded row shows, `None` off the
    /// expanded rows.
    pub fn expanded_row_message(&self, row: usize) -> Option<(ThreadId, usize)> {
        let (stub, index, _) = self.stub_on_row(row)?;
        if !stub.expanded {
            return None;
        }
        let message = stub.message_of_row(index)?;
        Some((stub.id, message))
    }

    /// The rendered row message `message` of the expanded `id` starts on.
    fn row_of_message(&self, id: &ThreadId, message: usize) -> Option<usize> {
        let (block, stub) = self
            .stubs()
            .into_iter()
            .enumerate()
            .find(|(_, stub)| stub.id() == id)?;
        let index = stub.row_of_message(message)?;
        self.view().row_of_stub_slot(block, index)
    }

    /// Expand `id` in place, the view staying where it is, and put the
    /// cursor on its newest message.
    pub fn expand_thread(&mut self, id: ThreadId) {
        self.refresh_watchers();
        self.expanded.insert(id.clone());
        self.place_stub_rows();
        let newest = self.newest_message(&id);
        self.set_thread_cursor_message(id, newest);
    }

    /// Fold `id` back to a stub.
    pub fn fold_thread(&mut self, id: &ThreadId) {
        if self.expanded.remove(id) {
            self.place_stub_rows();
        }
    }

    /// Put the text cursor on message `message` of `id`, expanding the
    /// thread if it is folded, so a reply lands under the reader's eye.
    pub(crate) fn goto_message(&mut self, id: ThreadId, message: usize) {
        if !self.is_expanded(&id) {
            self.expanded.insert(id.clone());
            self.place_stub_rows();
        }
        if let Some(row) = self.row_of_message(&id, message) {
            self.view_mut().goto_row(row);
        }
        self.set_thread_cursor_message(id, message);
    }

    /// `c` on a row threads cover: expand the thread cursor's thread; on
    /// an expanded thread, fold it and expand the next thread covering
    /// the same lines, in line order and wrapping, until the cycle comes
    /// back to where it started, when nothing is expanded.
    pub fn cycle_expanded(&mut self, covering: Vec<ThreadId>) {
        let Some(current) = self.thread_cursor().thread().cloned() else {
            return;
        };
        if !self.is_expanded(&current) {
            self.cycle = Some((current.clone(), covering));
            self.expand_thread(current);
            return;
        }
        self.fold_thread(&current);
        // The ring the cycle started with, unless it no longer holds the
        // thread; then the threads covering its lines now.
        let (start, ring) = self
            .cycle
            .take()
            .filter(|(_, ring)| ring.contains(&current))
            .unwrap_or((current.clone(), covering));
        let next = ring
            .iter()
            .position(|id| *id == current)
            .map(|at| ring[(at + 1) % ring.len()].clone());
        match next {
            Some(next) if next != start && next != current => {
                self.cycle = Some((start, ring));
                self.expand_thread(next);
            }
            _ => self.set_thread_cursor(current),
        }
    }

    /// `Space c z`: expand every stub in the file, or fold every expanded
    /// thread when any is.
    pub fn toggle_expand_all(&mut self) {
        let ids: Vec<ThreadId> = self.stubs().into_iter().map(|stub| stub.id).collect();
        if ids.iter().any(|id| self.expanded.contains(id)) {
            for id in &ids {
                self.expanded.remove(id);
            }
        } else {
            self.expanded.extend(ids);
        }
        self.place_stub_rows();
    }

    /// Expand the thread cursor's thread, as `c` does on a fresh row.
    #[cfg(test)]
    pub fn expand_at_cursor(&mut self) {
        if let Some(id) = self.thread_cursor().thread().cloned() {
            self.expand_thread(id);
        }
    }

    /// Whether the thread cursor's thread is expanded in place.
    #[cfg(test)]
    pub fn shows_thread(&self) -> bool {
        self.thread_cursor()
            .thread()
            .is_some_and(|id| self.is_expanded(id))
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

    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use crate::app::testing::{self, press, source_app};

    use crate::app::threads::ComposeTarget;
    use crate::app::{App, Focus, Popup};

    fn annotate(app: &mut App, from: usize, to: usize, text: &str) {
        app.view_mut().goto_source_line(from);
        if to > from {
            app.view_mut().select_lines();
            app.view_mut().move_down(to - from);
        }
        app.start_new_comment();
        app.compose_insert(text);
        app.compose_submit();
    }

    fn click(app: &mut App, column: u16, row: u16) {
        crate::app::input::mouse::handle_mouse(
            app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row,
                modifiers: KeyModifiers::NONE,
            },
        );
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
        let dir = testing::workspace("stubs-rows", testing::README)?;
        let mut app = source_app(&dir)?;
        annotate(&mut app, 3, 5, "outer thread");
        annotate(&mut app, 5, 5, "inner point");
        app.thread_reply();
        app.compose_insert("agent-free reply");
        app.compose_submit();
        // A reply from the text expands the thread; fold it again.
        let inner = app.file_threads()[1].clone();
        app.fold_thread(&inner);
        app.view_mut().goto_source_line(1);

        // Row model: 8 source rows, plus 1 + 2 rows under L5 and 1 under L7.
        annotate(&mut app, 7, 7, "seven");
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
        let (stub, index, last) = app
            .stub_on_row(5)
            .ok_or_else(|| anyhow::anyhow!("a stub under L5"))?;
        assert_eq!(stub.messages(), [0]);
        assert_eq!((index, last), (0, true));
        let (stub, index, last) = app
            .stub_on_row(6)
            .ok_or_else(|| anyhow::anyhow!("a stub under L5"))?;
        assert_eq!(stub.messages(), [0, 1]);
        assert_eq!((index, last), (0, false));
        assert_eq!(app.stub_on_row(7).map(|(_, i, l)| (i, l)), Some((1, true)));

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
        Ok(())
    }

    /// `Space c c` hides and shows every stub; resolved threads have none
    /// until `Space c x`.
    #[test]
    fn the_toggles_hide_stubs_and_resolved_ones() -> anyhow::Result<()> {
        let dir = testing::workspace("stubs-toggles", testing::README)?;
        let mut app = source_app(&dir)?;
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
        let dir = testing::workspace("stubs-detached", testing::README)?;
        let mut app = source_app(&dir)?;
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

    /// `c` expands the thread under the cursor in place without moving
    /// the view; `j`/`k` walk its messages and `r` replies with the
    /// cursor landing on the reply; `c` folds; `c` cycles through the
    /// threads covering a row and ends with none expanded.
    #[test]
    fn c_expands_in_place_walks_messages_and_cycles() -> anyhow::Result<()> {
        let dir = testing::workspace("stubs-expand", testing::README)?;
        let mut app = source_app(&dir)?;
        annotate(&mut app, 3, 5, "outer thread");
        annotate(&mut app, 5, 5, "inner point");
        app.thread_reply();
        app.compose_insert("first reply\nwith a second line");
        app.compose_submit();
        let outer = app.file_threads()[0].clone();
        let inner = app.file_threads()[1].clone();
        assert!(app.is_expanded(&inner), "a reply from the text expands");
        app.fold_thread(&inner);

        // On L5 the thread cursor is the inner thread; `c` expands it.
        app.view_mut().goto_source_line(5);
        let scroll = app.view().scroll();
        press(&mut app, "c");
        assert!(app.is_expanded(&inner));
        assert!(!app.is_expanded(&outer));
        assert_eq!(app.view().scroll(), scroll, "the view stays still");
        assert_eq!(app.view().cursor_source_line(), Some(5), "the cursor too");
        assert_eq!(app.thread_cursor().message(), 1, "the newest message");
        let shown = screen(&app)?;
        // Outer's collapsed stub, then inner's header, comment, reply.
        assert!(shown[5].contains("outer thread"), "{:?}", shown[5]);
        assert!(
            shown[6].contains("open") && shown[6].contains("fold"),
            "{:?}",
            shown[6]
        );
        assert!(
            shown[7].contains("user") && shown[8].contains("inner point"),
            "{:?}",
            &shown[7..9]
        );
        assert!(
            shown[10].contains("first reply") && shown[11].contains("second line"),
            "{:?}",
            &shown[10..12]
        );
        assert_eq!(shown[12].trim(), "6", "L6 follows: {:?}", shown[12]);

        // `j` from L5 stops on the comment, then the reply, then L6.
        press(&mut app, "j");
        assert_eq!(app.view().cursor().row, 7);
        assert_eq!(app.thread_cursor().message(), 0);
        press(&mut app, "j");
        assert_eq!(app.view().cursor().row, 9);
        assert_eq!(app.thread_cursor().message(), 1);
        press(&mut app, "j");
        assert_eq!(app.view().cursor_source_line(), Some(6));
        press(&mut app, "kk");
        assert_eq!(app.view().cursor().row, 7);
        assert_eq!(app.expanded_row_message(7), Some((inner.clone(), 0)));

        // `r` on a message row replies to its thread; the cursor lands on
        // the reply and the keys stay with the text.
        press(&mut app, "r");
        assert!(matches!(
            app.popup(),
            Some(Popup::Compose(c)) if *c.target() == ComposeTarget::Reply(inner.clone())
        ));
        app.compose_insert("second reply");
        app.compose_submit();
        assert_eq!(app.focus(), Focus::View);
        assert_eq!(app.thread_cursor().message(), 2);
        assert_eq!(
            app.expanded_row_message(app.view().cursor().row),
            Some((inner.clone(), 2))
        );
        // `e` edits the message under the cursor.
        press(&mut app, "e");
        assert!(matches!(
            app.popup(),
            Some(Popup::Compose(c)) if matches!(c.target(), ComposeTarget::Edit { .. })
        ));
        app.close_popup();

        // `c` on an expanded row folds it and expands the next covering
        // thread; the outer thread covers L5 too, so it comes next, and
        // after it `c` leaves nothing expanded.
        press(&mut app, "c");
        assert!(!app.is_expanded(&inner));
        assert!(app.is_expanded(&outer));
        press(&mut app, "c");
        assert!(!app.is_expanded(&outer));
        assert!(!app.is_expanded(&inner), "the cycle ends with none");
        assert_eq!(app.view().cursor_source_line(), Some(5));

        // `Space c z` expands every stub, then folds them all.
        press(&mut app, " cz");
        assert!(app.is_expanded(&inner) && app.is_expanded(&outer));
        press(&mut app, " cz");
        assert!(!app.is_expanded(&inner) && !app.is_expanded(&outer));

        // A click on a collapsed stub expands it with the cursor on it.
        let shown = screen(&app)?;
        let row = shown
            .iter()
            .position(|line| line.contains("inner point"))
            .ok_or_else(|| anyhow::anyhow!("inner's stub"))?;
        click(&mut app, 60, u16::try_from(row)?);
        assert!(app.is_expanded(&inner));
        assert_eq!(app.thread_cursor().thread(), Some(&inner));
        assert_eq!(
            app.expanded_row_message(app.view().cursor().row)
                .map(|(id, _)| id),
            Some(inner.clone())
        );
        // `dd` on its rows deletes the thread (ADR 0034).
        press(&mut app, "dd");
        assert_eq!(app.file_threads().len(), 1);
        assert_eq!(app.message(), Some("deleted"));
        Ok(())
    }
}
