// @okf-doc: /decisions/0049-inline-threads-and-the-rail.md
//! Inline stubs (ADR 0049): a thread shows under the last row of its
//! lines as one row, the first line of its newest message, and `z`
//! expands it in place into the whole thread and folds it again
//! (ADR 0065), so a file reads with its conversation
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
//! the app retains those stubs and each expanded message layout while
//! the view keeps the anchors, counts, and stops. Drawing and row lookup
//! reuse the retained data. The draft (ADR 0054) is rows too: a reply's
//! at the bottom of its thread's block, an edit's in place of the
//! message it edits, and a new comment's in a draft block of its own.

use std::sync::Arc;

use fathomable_core::annotations::{LineRange, Placement, Thread, ThreadId};
use fathomable_core::config::ThreadsConfig;
use fathomable_core::layout::RowAnchor;

use crate::app::App;
use crate::app::draw::message::{ExpandedLayout, MESSAGE_INDENT, MessageLayouts};
use crate::app::threads::{ComposeTarget, Mark, ThreadState};
use crate::app::view::StubBlock;

/// Messages a collapsed stub shows: the newest one (ADR 0049, amended
/// 2026-09-09).
const STUB_MESSAGES: usize = 1;

/// Whether stubs are drawn at all (`threads { stubs }`, `Space v t`), and
/// whether resolved threads get one (`Space v r`).
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

/// Reuse `id`'s layout when its thread revision and pane width still match.
fn take_layout(
    cached: &mut Vec<(ThreadId, ExpandedLayout)>,
    id: &ThreadId,
    thread: &Thread,
    width: usize,
    bodies: Arc<MessageLayouts>,
) -> ExpandedLayout {
    cached
        .iter()
        .position(|(cached_id, _)| cached_id == id)
        .map(|index| cached.swap_remove(index))
        .map(|(_, layout)| layout)
        .filter(|layout| layout.matches(thread, width))
        .unwrap_or_else(|| ExpandedLayout::new(width, bodies))
}

/// Fit the active reply or edit draft into an expanded thread's rows.
fn fit_draft(
    draft: Option<&(ComposeTarget, usize)>,
    id: &ThreadId,
    rows: &mut usize,
    stops: &mut [usize],
) -> Option<(usize, usize)> {
    let (target, draft_rows) = draft?;
    if target.thread() != Some(id) {
        return None;
    }
    let Some(message) = target.edited_message() else {
        let at = *rows;
        *rows += *draft_rows;
        return Some((at, 0));
    };
    let at = stops.get(message).copied()?;
    let end = stops.get(message + 1).copied().unwrap_or(*rows);
    let taken = end - at;
    for stop in stops.iter_mut().skip(message + 1) {
        *stop = *stop + *draft_rows - taken;
    }
    *rows = *rows + *draft_rows - taken;
    Some((at, taken))
}

impl App {
    /// Whether stubs are drawn (`threads { stubs }`).
    pub(crate) fn stubs_shown(&self) -> bool {
        self.stubs.shown
    }

    /// Whether resolved threads get a stub (`Space v r`).
    pub(crate) fn stubs_resolved(&self) -> bool {
        self.stubs.resolved
    }

    /// Whether `id` is expanded in place.
    pub(crate) fn is_expanded(&self, id: &ThreadId) -> bool {
        self.expanded.contains(id) || self.file_thread_peeked(id)
    }

    /// Build the current document's stubs in row order and reuse any
    /// unchanged expanded-message layouts.
    fn build_stubs(
        &self,
        mut cached_layouts: Vec<(ThreadId, ExpandedLayout)>,
    ) -> (Vec<Stub>, Vec<(ThreadId, ExpandedLayout)>) {
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
                        || matches!(mark.kind(), ThreadState::Active | ThreadState::Proposed)
                        || self.is_expanded(mark.id())))
                    || self.file_thread_peeked(mark.id())
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
        let mut expanded_layouts = Vec::with_capacity(marks.len());
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
                let expanded = self.is_expanded(mark.id());
                let (messages, rows, stops, slot) = if expanded {
                    let bodies = self.message_layout_cache.layout(
                        thread,
                        width.saturating_sub(MESSAGE_INDENT).max(1),
                        self.highlighter(),
                    );
                    let layout = take_layout(&mut cached_layouts, mark.id(), thread, width, bodies);
                    let mut rows = layout.rows();
                    let mut stops = layout.stops().to_vec();
                    expanded_layouts.push((mark.id().clone(), layout));
                    // The header row comes first; the stops follow it.
                    rows += 1;
                    for stop in &mut stops {
                        *stop += 1;
                    }
                    let slot = fit_draft(draft.as_ref(), mark.id(), &mut rows, &mut stops);
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
        expanded_layouts.extend(
            cached_layouts
                .into_iter()
                .filter(|(id, layout)| self.is_expanded(id) && layout.width() == width),
        );
        (stubs, expanded_layouts)
    }

    /// The already placed stubs for the current document.
    pub(crate) fn stubs(&self) -> &[Stub] {
        &self.inline_stubs
    }

    /// Lay the current document out again with its stub rows in place;
    /// called whenever the marks, the messages, or the toggles change.
    pub(crate) fn place_stub_rows(&mut self) {
        let cached_layouts = std::mem::take(&mut self.expanded_layout_cache);
        let (stubs, layouts) = self.build_stubs(cached_layouts);
        let blocks: Vec<StubBlock> = stubs.iter().map(Stub::block).collect();
        self.inline_stubs = stubs;
        self.expanded_layout_cache = layouts;
        self.sync_text_height();
        self.view_mut().set_stub_blocks(blocks);
    }

    /// The stub whose row `row` is, with the row's index in the block
    /// and whether it is the block's last row.
    pub(crate) fn stub_on_row(&self, row: usize) -> Option<(Stub, usize, bool)> {
        let (block, index) = self.view().stub_slot_of_row(row)?;
        let stub = self.inline_stubs.get(block)?.clone();
        let last = index + 1 == stub.rows;
        Some((stub, index, last))
    }

    /// The prepared message bodies for one expanded thread.
    pub(crate) fn expanded_layout(&self, id: &ThreadId) -> Option<&ExpandedLayout> {
        self.expanded_layout_cache
            .iter()
            .find(|(thread, _)| thread == id)
            .map(|(_, layout)| layout)
    }

    /// Whether the text cursor rests inside `id` rather than on source text.
    pub(crate) fn cursor_on_thread_row(&self, id: &ThreadId) -> bool {
        let row = self.view().cursor().row;
        self.stub_on_row(row)
            .is_some_and(|(stub, _, _)| stub.thread() == Some(id))
            || (self.view().detached_anchor_of_row(row).is_some()
                && self.file_thread_cursor().thread() == Some(id))
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
            .iter()
            .enumerate()
            .find(|(_, stub)| stub.thread() == Some(id))?;
        let index = stub.row_of_message(message)?;
        self.view().row_of_stub_slot(block, index)
    }

    /// The rendered row range occupied by one expanded message.
    pub(super) fn file_message_range(
        &self,
        id: &ThreadId,
        message: usize,
    ) -> Option<std::ops::Range<usize>> {
        let (block, stub) = self
            .stubs()
            .iter()
            .enumerate()
            .find(|(_, stub)| stub.thread() == Some(id))?;
        let start = stub.row_of_message(message)?;
        let end = stub
            .stops
            .iter()
            .copied()
            .find(|stop| *stop > start)
            .unwrap_or(stub.rows);
        let first = self.view().row_of_stub_slot(block, start)?;
        let last = self.view().row_of_stub_slot(block, end.saturating_sub(1))?;
        Some(first..last + 1)
    }

    /// Source-side edge through newest reply for contextual thread landing.
    pub(crate) fn file_thread_jump_span(
        &self,
        id: &ThreadId,
        message: usize,
    ) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
        let priority = self.file_message_range(id, message)?;
        let (block, _stub) = self
            .stubs()
            .iter()
            .enumerate()
            .find(|(_, stub)| stub.thread() == Some(id))?;
        let block_start = self.view().row_of_stub_slot(block, 0)?;
        let start = self
            .mark_of(id)
            .map_or(block_start, |mark| match mark.placement() {
                Placement::Anchored(range) | Placement::Edited(range) => self
                    .view()
                    .rendered_row_of_source_line(range.start())
                    .unwrap_or(block_start),
                Placement::Detached(_) => block_start.saturating_sub(1),
                Placement::File => block_start,
            });
        Some((start..priority.end, priority))
    }

    /// Expand `id` in place, the view staying where it is, and put the
    /// cursor on its newest message.
    pub(crate) fn expand_thread(&mut self, id: ThreadId) {
        self.release_file_peek(&id);
        self.expanded.insert(id.clone());
        self.place_stub_rows();
        let newest = self.newest_message(&id);
        self.set_file_thread_cursor(id, newest);
    }

    /// Fold `id` back to a stub.
    pub(crate) fn fold_thread(&mut self, id: &ThreadId) {
        let peeked = self.file_thread_peeked(id);
        self.release_file_peek(id);
        if self.expanded.remove(id) || peeked {
            self.place_stub_rows();
        }
    }

    /// Put the text cursor on message `message` of `id`, expanding the
    /// thread if it is folded, so a reply lands under the reader's eye.
    pub(crate) fn goto_message(&mut self, id: ThreadId, message: usize) {
        if self.focus == crate::app::Focus::Review {
            self.set_review_thread_cursor(id, message);
            return;
        }
        if self.focus == crate::app::Focus::ThreadsPane {
            self.set_threads_pane_cursor(&id, message);
            if self.review_list().is_open() {
                if self.review_thread_cursor().thread() == Some(&id) {
                    self.set_review_thread_cursor(id, message);
                }
                return;
            }
        }
        self.seat_file_message(id, message);
    }

    /// Seat File on one logical message without changing another surface's
    /// independent thread cursor.
    pub(super) fn seat_file_message(&mut self, id: ThreadId, message: usize) {
        if !self.is_expanded(&id) {
            self.expanded.insert(id.clone());
            self.place_stub_rows();
        }
        if let Some(row) = self.row_of_message(&id, message) {
            self.view_mut().goto_row(row);
        }
        self.set_file_thread_cursor(id, message);
    }

    /// Expand the thread cursor's thread.
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

    /// `Space v r`: give resolved threads a stub too, or not.
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
mod tests;
