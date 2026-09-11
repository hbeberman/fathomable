// @okf-doc: /decisions/0049-inline-threads-and-the-rail.md
//! Inline stubs (ADR 0049): a thread shows under the last row of its
//! lines as one row, the first line of its newest message, and `c` or
//! `z` expands it in place into the whole thread (`z`
//! folds it again, ADR 0065), so a file reads with its conversation
//! where the lines are.
//!
//! Stubs are not lines. The view inserts their rows after the row they
//! hang under, the way a detached thread's row is inserted (ADR 0039).
//! A collapsed stub's row is a stop (ADR 0076): `j`/`k` land on it, the
//! thread cursor is its thread, and `z` expands it. An expanded
//! thread's message rows are stops too: `j`/`k` walk them, and the
//! message under the cursor is the thread cursor's. Which
//! threads produce rows, under which row, in what order, with which
//! messages, and which rows are stops is decided here from the marks;
//! the view keeps only the anchors, counts, and stops, and the drawing
//! asks back for the words. The draft (ADR 0054) is rows too: a reply's
//! at the bottom of its thread's block, an edit's in place of the
//! message it edits, and a new comment's in a draft block of its own.

use fathomable_core::annotations::{LineRange, Placement, ThreadId};
use fathomable_core::config::ThreadsConfig;
use fathomable_core::layout::RowAnchor;

use crate::app::App;
use crate::app::draw::message::expanded_rows;
use crate::app::threads::{ComposeTarget, Mark, ThreadState};
use crate::app::view::StubBlock;

/// Messages a collapsed stub shows: the newest one (ADR 0049, amended
/// 2026-09-09).
const STUB_MESSAGES: usize = 1;

/// Whether stubs are drawn at all (`threads { stubs }`, `Space v t`), and
/// whether resolved threads get one (`Space v x`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StubState {
    pub(crate) shown: bool,
    pub(crate) resolved: bool,
}

impl StubState {
    pub(crate) fn from_config(config: &ThreadsConfig) -> Self {
        Self {
            shown: config.stubs,
            resolved: config.stubs_resolved,
        }
    }
}

/// What a block of rows stands for: a thread, or the comment being
/// written on lines that have no thread yet (ADR 0054).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Subject {
    Thread(ThreadId),
    Draft(LineRange),
    /// The comment being written on the file as a whole (ADR 0063).
    FileDraft,
}

/// One block's stub: what it stands for, where it hangs, which messages
/// it shows (oldest first, zero the comment), and its shape when
/// expanded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stub {
    subject: Subject,
    anchor: RowAnchor,
    messages: Vec<usize>,
    expanded: bool,
    /// Rows the block takes.
    rows: usize,
    /// Row indices within the block the cursor may rest on: an expanded
    /// thread's message rows. Empty for a collapsed stub.
    stops: Vec<usize>,
    /// The block row of the draft's author row, while this block holds
    /// the draft (ADR 0054), and the rows of the message it stands in
    /// for: none for a reply or a new comment.
    draft: Option<(usize, usize)>,
}

impl Stub {
    /// What the block stands for.
    pub(crate) fn subject(&self) -> &Subject {
        &self.subject
    }

    /// The thread the block shows, `None` for a draft block.
    pub(crate) fn thread(&self) -> Option<&ThreadId> {
        match &self.subject {
            Subject::Thread(id) => Some(id),
            Subject::Draft(_) | Subject::FileDraft => None,
        }
    }

    /// The block row of the draft's author row, when the draft is here.
    pub(crate) fn draft_row(&self) -> Option<usize> {
        self.draft.map(|(row, _)| row)
    }

    /// The draft's author row and the rows of the message it replaces,
    /// when the draft is here.
    pub(crate) fn draft_slot(&self) -> Option<(usize, usize)> {
        self.draft
    }

    /// The messages shown, as indices into the thread: the comment is 0.
    #[cfg(test)]
    pub(crate) fn messages(&self) -> &[usize] {
        &self.messages
    }

    /// Whether the whole thread shows.
    pub(crate) fn expanded(&self) -> bool {
        self.expanded
    }

    /// The message row `index` of the block belongs to: for a collapsed
    /// stub the message on that row, for an expanded thread the message
    /// whose header or body it is; `None` on the expanded header row.
    #[must_use]
    pub(crate) fn message_of_row(&self, index: usize) -> Option<usize> {
        if !self.expanded {
            return self.messages.get(index).copied();
        }
        let position = self.stops.iter().rposition(|&stop| stop <= index)?;
        self.messages.get(position).copied()
    }

    /// The row within the block that message `message` starts on, when
    /// the thread is expanded.
    #[must_use]
    pub(crate) fn row_of_message(&self, message: usize) -> Option<usize> {
        if !self.expanded {
            return None;
        }
        let position = self.messages.iter().position(|&m| m == message)?;
        self.stops.get(position).copied()
    }

    /// The block as the view lays it out.
    pub(crate) fn block(&self) -> StubBlock {
        StubBlock {
            anchor: self.anchor,
            rows: self.rows,
            stops: self.stops.clone(),
            expanded: self.expanded,
        }
    }
}

/// The draft block of a new comment: on its lines, or above the first
/// line for a comment on the file as a whole; `None` for a reply or an
/// edit, which are written in their thread's block.
fn new_comment_block(target: &ComposeTarget, draft_rows: usize) -> Option<Stub> {
    let (subject, anchor) = match target {
        ComposeTarget::New(range) => (Subject::Draft(*range), RowAnchor::Line(range.end())),
        ComposeTarget::OnFile => (Subject::FileDraft, RowAnchor::Top),
        ComposeTarget::Reply(_) | ComposeTarget::Edit { .. } => return None,
    };
    Some(Stub {
        subject,
        anchor,
        messages: Vec::new(),
        expanded: true,
        rows: 1 + draft_rows,
        stops: Vec::new(),
        draft: Some((1, 0)),
    })
}

impl App {
    /// Whether stubs are drawn (`threads { stubs }`).
    #[cfg(test)]
    pub(crate) fn stubs_shown(&self) -> bool {
        self.stubs.shown
    }

    /// Whether resolved threads get a stub (`Space v x`).
    #[cfg(test)]
    pub(crate) fn stubs_resolved(&self) -> bool {
        self.stubs.resolved
    }

    /// Whether `id` is expanded in place.
    pub(crate) fn is_expanded(&self, id: &ThreadId) -> bool {
        self.expanded.contains(id)
    }

    /// The current document's stubs in row order: threads by start line,
    /// then end line, then id, so stacked stubs come one thread after
    /// another and never interleave.
    pub(crate) fn stubs(&self) -> Vec<Stub> {
        let width = self.view().layout().width();
        let draft = self
            .draft()
            .map(|compose| (compose.target().clone(), self.draft_rows()));
        let holds_draft = |id: &ThreadId| {
            draft
                .as_ref()
                .is_some_and(|(target, _)| target.thread() == Some(id))
        };
        let mut marks: Vec<&Mark> = self
            .marks()
            .iter()
            .filter(|mark| {
                // A resolved thread has no stub unless asked for, but an
                // expanded one always has its rows, and the thread the
                // draft is written in has them even with stubs hidden.
                (self.stubs.shown
                    && (self.stubs.resolved
                        || matches!(mark.kind(), ThreadState::Open | ThreadState::Waiting)
                        || self.expanded.contains(mark.id())))
                    || holds_draft(mark.id())
            })
            .collect();
        // A thread on the file as a whole has no lines and comes first
        // (ADR 0063).
        marks.sort_by_key(|mark| {
            let range = mark.range();
            (
                range.map(|range| range.start()),
                range.map(|range| range.end()),
                mark.id().clone(),
            )
        });
        let mut stubs: Vec<Stub> = marks
            .into_iter()
            .filter_map(|mark| {
                let thread = self.thread(mark.id())?;
                let anchor = match mark.placement() {
                    Placement::Detached(_) => RowAnchor::Detached(self.detached_anchor(mark)),
                    Placement::Anchored(range) | Placement::Edited(range) => {
                        RowAnchor::Line(range.end())
                    }
                    Placement::File => RowAnchor::Top,
                };
                let count = thread.replies().len() + 1;
                let expanded = self.expanded.contains(mark.id());
                let (messages, rows, stops, slot) = if expanded {
                    let (rows, stops) = expanded_rows(thread, width, self.highlighter());
                    // The header row comes first; the stops follow it.
                    let mut rows = rows + 1;
                    let mut stops: Vec<usize> = stops.into_iter().map(|stop| stop + 1).collect();
                    let slot = match &draft {
                        Some((target, draft_rows)) if target.thread() == Some(mark.id()) => {
                            match target.edited_message() {
                                // A reply is written after the last message.
                                None => {
                                    let at = rows;
                                    rows += draft_rows;
                                    Some((at, 0))
                                }
                                // An edit stands in for its message; the
                                // stops after it move by the difference.
                                Some(message) => {
                                    let at = stops[message];
                                    let end = stops.get(message + 1).copied().unwrap_or(rows);
                                    let taken = end - at;
                                    for stop in stops.iter_mut().skip(message + 1) {
                                        *stop = *stop + draft_rows - taken;
                                    }
                                    rows = rows + draft_rows - taken;
                                    Some((at, taken))
                                }
                            }
                        }
                        _ => None,
                    };
                    ((0..count).collect(), rows, stops, slot)
                } else {
                    let messages: Vec<usize> =
                        (count.saturating_sub(STUB_MESSAGES)..count).collect();
                    let rows = messages.len();
                    // Every row of a collapsed stub is a stop (ADR 0076).
                    (messages, rows, (0..rows).collect(), None)
                };
                Some(Stub {
                    subject: Subject::Thread(mark.id().clone()),
                    anchor,
                    messages,
                    expanded,
                    rows,
                    stops,
                    draft: slot,
                })
            })
            .collect();
        // A new comment's draft block hangs under its lines, after any
        // thread's stub on the same row (ADR 0054); a comment on the file
        // as a whole is written above the first line, after any file
        // thread's stub (ADR 0063).
        if let Some((target, draft_rows)) = draft {
            stubs.extend(new_comment_block(&target, draft_rows));
        }
        stubs
    }

    /// Lay the current document out again with its stub rows in place;
    /// called whenever the marks, the messages, or the toggles change.
    pub(crate) fn place_stub_rows(&mut self) {
        let blocks: Vec<StubBlock> = self.stubs().iter().map(Stub::block).collect();
        self.view_mut().set_stub_blocks(blocks);
    }

    /// The stub whose row `row` is, with the row's index in the block
    /// and whether it is the block's last row.
    pub(crate) fn stub_on_row(&self, row: usize) -> Option<(Stub, usize, bool)> {
        let (block, index) = self.view().stub_slot_of_row(row)?;
        let stub = self.stubs().into_iter().nth(block)?;
        let last = index + 1 == stub.rows;
        Some((stub, index, last))
    }

    /// The thread and message an expanded row shows, `None` off the
    /// expanded rows.
    pub(crate) fn expanded_row_message(&self, row: usize) -> Option<(ThreadId, usize)> {
        let (stub, index, _) = self.stub_on_row(row)?;
        if !stub.expanded {
            return None;
        }
        let message = stub.message_of_row(index)?;
        Some((stub.thread()?.clone(), message))
    }

    /// The rendered row message `message` of the expanded `id` starts on.
    fn row_of_message(&self, id: &ThreadId, message: usize) -> Option<usize> {
        let (block, stub) = self
            .stubs()
            .into_iter()
            .enumerate()
            .find(|(_, stub)| stub.thread() == Some(id))?;
        let index = stub.row_of_message(message)?;
        self.view().row_of_stub_slot(block, index)
    }

    /// Expand `id` in place, the view staying where it is, and put the
    /// cursor on its newest message.
    pub(crate) fn expand_thread(&mut self, id: ThreadId) {
        self.refresh_watchers();
        self.expanded.insert(id.clone());
        self.place_stub_rows();
        let newest = self.newest_message(&id);
        self.set_thread_cursor_message(id, newest);
    }

    /// Fold `id` back to a stub.
    pub(crate) fn fold_thread(&mut self, id: &ThreadId) {
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
    pub(crate) fn cycle_expanded(&mut self, covering: Vec<ThreadId>) {
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

    /// Expand the thread cursor's thread, as `c` does on a fresh row.
    #[cfg(test)]
    pub(crate) fn expand_at_cursor(&mut self) {
        if let Some(id) = self.thread_cursor().thread().cloned() {
            self.expand_thread(id);
        }
    }

    /// Whether the thread cursor's thread is expanded in place.
    #[cfg(test)]
    pub(crate) fn shows_thread(&self) -> bool {
        self.thread_cursor()
            .thread()
            .is_some_and(|id| self.is_expanded(id))
    }

    /// `Space v t`: draw stubs under threads' lines, or not (ADR 0060);
    /// the runtime switch for `threads { stubs }`.
    pub(crate) fn toggle_stubs(&mut self) {
        self.stubs.shown = !self.stubs.shown;
        self.place_stub_rows();
        self.push_toast(if self.stubs.shown {
            "stubs shown".to_owned()
        } else {
            "stubs hidden".to_owned()
        });
    }

    /// `Space v x`: give resolved threads a stub too, or not.
    pub(crate) fn toggle_resolved_stubs(&mut self) {
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
    use crossterm::event::KeyCode;
    use std::fs;

    use crate::app::testing::{self, click, press, press_key, screen, source_app};

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

    /// The stub rows among `rows` whose text, the row's last cell that
    /// is not blank, reads bold: the thread cursor's (ADR 0067).
    fn bold_rows(app: &App, rows: &[usize]) -> anyhow::Result<Vec<usize>> {
        let buffer = testing::buffer(app)?;
        Ok(rows
            .iter()
            .copied()
            .filter(|&row| {
                let y = u16::try_from(row).unwrap_or(u16::MAX);
                (0..buffer.area.width)
                    .rev()
                    .find(|&x| buffer[(x, y)].symbol() != " ")
                    .is_some_and(|x| {
                        buffer[(x, y)]
                            .modifier
                            .contains(ratatui::style::Modifier::BOLD)
                    })
            })
            .collect())
    }

    /// Stub rows sit under the last row of their thread, carry no line
    /// number, and stacked threads come one after another in line order;
    /// the cursor stops on each (ADR 0076) and the hint marks only the
    /// cursor's.
    /// Two threads stacked under L5: `outer` on L3-5 and `inner` on L5
    /// with one reply, `reply`, the inner one folded back to a stub
    /// after the reply expanded it.
    fn stacked_threads(
        app: &mut crate::app::App,
        reply: &str,
    ) -> (
        fathomable_core::annotations::ThreadId,
        fathomable_core::annotations::ThreadId,
    ) {
        annotate(app, 3, 5, "outer thread");
        annotate(app, 5, 5, "inner point");
        app.thread_reply();
        app.compose_insert(reply);
        app.compose_submit();
        let outer = app.file_threads()[0].clone();
        let inner = app.file_threads()[1].clone();
        assert!(app.is_expanded(&inner), "a reply from the text expands");
        app.fold_thread(&inner);
        (outer, inner)
    }

    #[test]
    fn stubs_hang_under_their_lines_and_the_cursor_stops_on_them() -> anyhow::Result<()> {
        let dir = testing::workspace("stubs-rows", testing::README)?;
        let mut app = source_app(&dir)?;
        let (outer, inner) = stacked_threads(&mut app, "agent-free reply");
        app.view_mut().goto_source_line(1);

        // Row model: 8 source rows, plus 1 + 1 rows under L5 and 1 under L7.
        annotate(&mut app, 7, 7, "seven");
        let rows = app.view().layout().lines().len();
        assert_eq!(rows, 11);
        assert_eq!(app.view().source_line_of_row(4), Some(5));
        assert!(
            app.view().stub_slot_of_row(5).is_some(),
            "outer's stub under L5"
        );
        assert!(
            app.view().stub_slot_of_row(6).is_some(),
            "inner's stub, its newest message"
        );
        assert_eq!(app.view().source_line_of_row(7), Some(6));
        let (stub, index, last) = app
            .stub_on_row(5)
            .ok_or_else(|| anyhow::anyhow!("a stub under L5"))?;
        assert_eq!(stub.messages(), [0]);
        assert_eq!((index, last), (0, true));
        let (stub, index, last) = app
            .stub_on_row(6)
            .ok_or_else(|| anyhow::anyhow!("a stub under L5"))?;
        assert_eq!(stub.messages(), [1], "the reply, not the comment");
        assert_eq!((index, last), (0, true));

        // The screen: the stub rows show the author, age, and text, no
        // number; the text of the thread under the cursor and the hint.
        app.view_mut().goto_source_line(5);
        let shown = screen(&app)?;
        assert!(shown[4].contains("gamma"), "{:?}", shown[4]);
        assert!(
            shown[5].contains("User") && shown[5].contains("outer thread"),
            "{:?}",
            shown[5]
        );
        assert!(
            !shown[5].contains(" 5 ") && !shown[5].contains(" 6 "),
            "no line number: {:?}",
            shown[5]
        );
        assert!(shown[6].contains("agent-free reply"), "{:?}", shown[6]);
        assert!(!shown[6].contains("inner point"), "{:?}", shown[6]);
        assert_eq!(shown[7].trim(), "6", "L6 follows: {:?}", shown[7]);
        // The cursor is on L5, which starts the inner thread: its stub
        // row reads bold, the outer's does not, and the bar says `z
        // expand` (ADR 0067).
        assert_eq!(bold_rows(&app, &[5, 6])?, [6], "the inner thread's row");
        assert!(
            shown[app.text_bar_row()].contains("z expand"),
            "{:?}",
            shown[app.text_bar_row()]
        );
        assert!(!shown[6].contains("(z expand)"), "{:?}", shown[6]);
        // On L4 only the outer thread covers the cursor.
        app.view_mut().goto_source_line(4);
        assert_eq!(bold_rows(&app, &[5, 6])?, [5], "the outer thread's row");

        // `j` from L5 stops on the outer stub, then the inner, then L6
        // (ADR 0076), each stub's thread the cursor's; `k` comes back
        // the same way.
        app.view_mut().goto_source_line(5);
        press(&mut app, "j");
        assert_eq!(app.view().cursor().row, 5);
        assert_eq!(app.thread_cursor().thread(), Some(&outer));
        press(&mut app, "j");
        assert_eq!(app.view().cursor().row, 6);
        assert_eq!(app.thread_cursor().thread(), Some(&inner));
        press(&mut app, "j");
        assert_eq!(app.view().cursor_source_line(), Some(6));
        assert_eq!(app.view().cursor().row, 7);
        press(&mut app, "kkk");
        assert_eq!(app.view().cursor().row, 4);
        press(&mut app, "G");
        assert_eq!(
            app.view().source_line_of_row(app.view().cursor().row),
            Some(8)
        );
        press(&mut app, "gg");
        assert_eq!(app.view().cursor().row, 0);
        // `x` selects lines, never a stub: from L5 the second press
        // steps over both stubs to L6, and from a stub the selection
        // anchors on the line the stub hangs under.
        app.view_mut().goto_source_line(5);
        press(&mut app, "xx");
        assert_eq!(
            app.view().selected_lines().map(|r| (r.start(), r.end())),
            Some((5, 6))
        );
        press_key(&mut app, KeyCode::Esc);
        app.view_mut().goto_source_line(5);
        press(&mut app, "j");
        assert_eq!(app.view().cursor().row, 5, "on the outer stub");
        press(&mut app, "x");
        assert_eq!(
            app.view().selected_lines().map(|r| (r.start(), r.end())),
            Some((5, 5)),
            "the line it hangs under"
        );
        press_key(&mut app, KeyCode::Esc);
        Ok(())
    }

    /// `threads { stubs #false }` and `Space v t` draw no stubs; resolved
    /// threads have none until `Space v x`.
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
        press(&mut app, " vx");
        assert_eq!(app.view().layout().lines().len(), 10);
        assert!(app.stubs_resolved());
        press(&mut app, " vx");
        assert_eq!(app.view().layout().lines().len(), 9);
        press(&mut app, " vt");
        assert!(!app.stubs_shown());
        assert_eq!(app.view().layout().lines().len(), 8);
        assert!(app.note_on_row(2).is_some(), "the gutter mark stays");
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
    /// Opening a thread from its chevron and closing it again leaves the
    /// view where it was: the row the stub hangs under keeps its place on
    /// screen, even though the cursor sat on a message row that went.
    #[test]
    fn opening_and_closing_a_thread_leaves_the_view_still() -> anyhow::Result<()> {
        let lines: Vec<String> = (1..=80).map(|n| format!("line {n}")).collect();
        let text = lines.join("\n") + "\n";
        let dir = testing::workspace("stubs-fold-still", &text)?;
        let mut app = source_app(&dir)?;
        annotate(&mut app, 40, 40, "a point\nwith a second line");
        app.thread_reply();
        app.compose_insert("a reply");
        app.compose_submit();
        let id = app.file_threads()[0].clone();
        app.fold_thread(&id);
        app.view_mut().goto_source_line(40);
        // The wheel brings the cursor to mid-screen, where the opened
        // thread fits below it without a scroll.
        app.view_mut().scroll_by(10);
        let scroll = app.view().scroll();
        assert!(scroll > 0, "the view has scrolled down to L40");
        assert_eq!(app.view().cursor_source_line(), Some(40));
        for _ in 0..3 {
            let newest = app.newest_message(&id);
            app.goto_message(id.clone(), newest);
            assert!(app.expanded_row_message(app.view().cursor().row).is_some());
            app.fold_thread(&id);
            assert_eq!(app.view().scroll(), scroll, "the view stays still");
            assert_eq!(app.view().cursor_source_line(), Some(40));
        }
        // `c` then `j` onto a message, and `z` to fold, the same.
        press(&mut app, "c");
        press(&mut app, "j");
        press(&mut app, "z");
        assert!(!app.is_expanded(&id));
        assert_eq!(app.view().scroll(), scroll, "the view stays still");
        Ok(())
    }

    #[test]
    fn c_expands_in_place_walks_messages_and_cycles() -> anyhow::Result<()> {
        let dir = testing::workspace("stubs-expand", testing::README)?;
        let mut app = source_app(&dir)?;
        let (outer, inner) = stacked_threads(&mut app, "first reply\nwith a second line");

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
            shown[6].contains("open") && !shown[6].contains("fold"),
            "the header is words alone (ADR 0067): {:?}",
            shown[6]
        );
        assert!(
            shown[app.text_bar_row()].contains("z fold"),
            "the bar names the key: {:?}",
            shown[app.text_bar_row()]
        );
        assert!(
            shown[7].contains("User") && shown[8].contains("inner point"),
            "{:?}",
            &shown[7..9]
        );
        assert!(
            shown[10].contains("first reply") && shown[11].contains("second line"),
            "{:?}",
            &shown[10..12]
        );
        assert_eq!(shown[12].trim(), "6", "L6 follows: {:?}", shown[12]);

        // `j` from L5 stops on the outer stub (ADR 0076), the comment,
        // then the reply, then L6.
        press(&mut app, "j");
        assert_eq!(app.view().cursor().row, 5);
        assert_eq!(app.thread_cursor().thread(), Some(&outer));
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

        // Expanding every stub, then folding them all.
        app.toggle_expand_all();
        assert!(app.is_expanded(&inner) && app.is_expanded(&outer));
        app.toggle_expand_all();
        assert!(!app.is_expanded(&inner) && !app.is_expanded(&outer));

        // A click on a stub's chevron column expands it with the cursor
        // on it (ADR 0073); the stub shows inner's newest two messages.
        let row = screen(&app)?
            .iter()
            .position(|line| line.contains("second reply") && !line.contains("inner point"))
            .ok_or_else(|| anyhow::anyhow!("inner's stub"))?;
        let chevron = app.sidebar_width() + crate::app::draw::gutter_width(app.view()) + 1;
        click(&mut app, chevron, row);
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
