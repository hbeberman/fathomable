// @okf-doc: /decisions/0010-viewer-ux.md
//! Viewer state: cursor, scrolling, selection, search, and reload anchoring.
//!
//! Everything here is plain data so the ADR 0010 conventions can be tested
//! without a terminal; `ui` draws it and `keys` drives it.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fathomable_core::annotations::LineRange;
use fathomable_core::diff::{Compare, Diff, LineStatus};
use fathomable_core::highlight::Highlighter;
use fathomable_core::layout::{Face, Layout, LineIndex, RowAnchor, display_width};
use regex::Regex;

#[cfg(test)]
use super::diff::Side;
use super::diff::{DiffBody, DiffView, Text};

mod navigation;

/// Rendered lines kept visible above and below the cursor.
const SCROLLOFF: usize = 3;

/// Editing-style mode shown in the status pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Normal,
    Select,
    Command,
    Search { backward: bool },
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Normal => "NOR",
            Self::Select => "SEL",
            Self::Command => "CMD",
            Self::Search { .. } => "SRCH",
        })
    }
}

/// Which layout of the document the pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Display {
    /// Rendered Markdown.
    #[default]
    Rendered,
    /// The raw source (`Space v s`, ADR 0010).
    Source,
    /// A unified diff between two sides (ADR 0060): `HEAD`, the index,
    /// last-seen snapshot, a checkpoint, a commit, or the working file.
    Diff,
}

/// A position in rendered coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub(crate) struct Cursor {
    pub(crate) row: usize,
    pub(crate) col: usize,
}

/// A selection between an anchor and a moving head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Selection {
    pub(crate) anchor: Cursor,
    pub(crate) head: Cursor,
    /// Whole lines (`V`) rather than columns (mouse drag).
    pub(crate) linewise: bool,
}

impl Selection {
    /// The selection ordered top-left to bottom-right.
    pub(crate) fn ordered(&self) -> (Cursor, Cursor) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Whether rendered cell `(row, col)` is inside the selection.
    pub(crate) fn contains(&self, row: usize, col: usize) -> bool {
        let (start, end) = self.ordered();
        if row < start.row || row > end.row {
            return false;
        }
        if self.linewise {
            return true;
        }
        let after_start = row > start.row || col >= start.col;
        let before_end = row < end.row || col <= end.col;
        after_start && before_end
    }
}

/// A search match in rendered coordinates: row and column span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Match {
    pub(crate) row: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// What the event loop must do after handling input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    None,
    Quit,
    Copy(String),
    /// Open a URL with the desktop's opener (`gf`, ADR 0052).
    Open(String),
    /// A `:` command the app handles.
    Command(String),
    /// Hand the comment draft to `$EDITOR` (ADR 0018).
    EditDraft,
}

/// What `]g` / `[g` did (ADR 0017), so the app can cross into the next
/// dirty file when the hunks of this one run out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HunkStep {
    /// The cursor moved to another hunk in this file.
    Moved,
    /// The only next hunk is back at the other end of the file.
    Wrapped,
    /// The file has no hunks against `HEAD`.
    Clean,
    /// Not in a git repository.
    NoBase,
}

/// How a view colours and initially displays its text (ADR 0016).
#[derive(Debug, Clone)]
pub(crate) struct Syntax {
    /// The shared highlighter.
    pub(crate) highlighter: Arc<Highlighter>,
    /// Language hint for the source layout: the file extension, or empty.
    pub(crate) hint: String,
    /// Whether the file opens rendered as Markdown rather than as source.
    pub(crate) markdown: bool,
}

impl Syntax {
    /// Plain Markdown: no colours, rendered first.
    #[must_use]
    pub(crate) fn plain() -> Self {
        Self {
            highlighter: Arc::new(Highlighter::plain()),
            hint: String::new(),
            markdown: true,
        }
    }
}

#[derive(Debug)]
pub(crate) struct View {
    text: String,
    layout: Layout,
    syntax: Syntax,
    width: usize,
    height: usize,
    display: Display,
    /// The last-seen snapshot (ADR 0015), `None` when the file has never
    /// been seen.
    seen: Option<String>,
    /// The file as staged in the index (ADR 0017), `None` outside git.
    index: Option<String>,
    /// The file as committed at `HEAD` (ADR 0006), `None` outside git.
    head: Option<String>,
    /// Absent Git endpoints whose retained display text must not replace them.
    missing: Missing,
    /// The diff's sides while it is shown (ADR 0060).
    diff_shown: Option<DiffView>,
    /// How diffs are compared and listed (ADR 0060).
    compare: Compare,
    /// The working tree against `HEAD`: the gutter, `]g`, and the counts.
    diff: Option<Diff>,
    /// The working tree against the index: which hunks are not yet staged.
    unstaged: Option<Diff>,
    /// The last time the reader moved, searched, or selected here.
    activity: Instant,
    cursor: Cursor,
    want_col: usize,
    scroll: usize,
    mode: Mode,
    input: String,
    selection: Option<Selection>,
    pattern: Option<Regex>,
    backward: bool,
    matches: Vec<Match>,
    message: Option<String>,
    changed: bool,
    /// Source lines a detached thread's row stands before (ADR 0039).
    detached: Vec<usize>,
    /// The stub blocks hanging under rows, in row order (ADR 0049).
    stubs: Vec<StubBlock>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Missing {
    index: bool,
    worktree: bool,
}

/// A block of rows inserted under a row for a thread (ADR 0049): where
/// it hangs, how many rows, and which of them the cursor may rest on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StubBlock {
    pub(crate) anchor: RowAnchor,
    pub(crate) rows: usize,
    /// Row indices within the block that are stops: an expanded thread's
    /// message rows, or a collapsed stub's rows (ADR 0076).
    pub(crate) stops: Vec<usize>,
    /// The thread is expanded in place; a collapsed stub's rows are
    /// stops a selection never takes.
    pub(crate) expanded: bool,
}

/// The cursor's place on a stub block, kept across a relayout: the
/// block's anchor, which of the blocks under that anchor it is, and the
/// row within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StubSeat {
    anchor: RowAnchor,
    ordinal: usize,
    index: usize,
}

impl View {
    /// Lay `text` out for a text area of `width` by `height` cells, as
    /// uncoloured Markdown.
    pub(crate) fn new(text: String, width: usize, height: usize) -> Self {
        Self::with_syntax(text, width, height, Syntax::plain())
    }

    /// Lay `text` out for a text area of `width` by `height` cells,
    /// coloured and initially displayed as `syntax` says.
    pub(crate) fn with_syntax(text: String, width: usize, height: usize, syntax: Syntax) -> Self {
        let display = if syntax.markdown {
            Display::Rendered
        } else {
            Display::Source
        };
        let layout = match display {
            Display::Rendered => Layout::render_with(&text, width, &syntax.highlighter),
            _ => Layout::source_with(&text, width, &syntax.hint, &syntax.highlighter),
        };
        Self {
            text,
            layout,
            syntax,
            width,
            height: height.max(1),
            display,
            seen: None,
            index: None,
            head: None,
            missing: Missing::default(),
            diff_shown: None,
            compare: Compare::default(),
            diff: None,
            unstaged: None,
            activity: Instant::now(),
            cursor: Cursor::default(),
            want_col: 0,
            scroll: 0,
            mode: Mode::Normal,
            input: String::new(),
            selection: None,
            pattern: None,
            backward: false,
            matches: Vec::new(),
            message: None,
            changed: false,
            detached: Vec::new(),
            stubs: Vec::new(),
        }
    }

    /// Lay the document out again with a blank row before each line in
    /// `lines`, for the detached threads anchored there (ADR 0039); a
    /// call that changes nothing keeps the layout.
    pub(crate) fn set_detached_anchors(&mut self, mut lines: Vec<usize>) {
        lines.sort_unstable();
        lines.dedup();
        if lines == self.detached {
            return;
        }
        self.detached = lines;
        self.relayout();
    }

    /// Lay the document out again with the stub rows of `blocks` under
    /// their anchors (ADR 0049); a call that changes nothing keeps the
    /// layout.
    pub(crate) fn set_stub_blocks(&mut self, blocks: Vec<StubBlock>) {
        if blocks == self.stubs {
            return;
        }
        let seat = self.stub_seat();
        self.stubs = blocks;
        self.relayout_seated(seat);
    }

    /// Where the cursor sits within a stub block, as it survives a
    /// relayout: the block's anchor, its place among the blocks under
    /// that anchor, and the row within it. `None` off the stub rows.
    fn stub_seat(&self) -> Option<StubSeat> {
        let (block, index) = self.stub_slot_of_row(self.cursor.row)?;
        let anchor = self.stubs.get(block)?.anchor;
        let ordinal = self.stubs[..block]
            .iter()
            .filter(|other| other.anchor == anchor)
            .count();
        Some(StubSeat {
            anchor,
            ordinal,
            index,
        })
    }

    /// The row `seat` names in the current layout, the cursor kept on
    /// the block it was on: on the same row when that is a stop, else
    /// on the stop before it within the block, else on the block's first
    /// row, the stub or the header, as a click rests there (ADR 0073,
    /// amended 2026-09-10). `None` when the block is gone.
    fn seat_row(&self, seat: StubSeat) -> Option<usize> {
        let (block, rows) = self
            .stubs
            .iter()
            .enumerate()
            .filter(|(_, other)| other.anchor == seat.anchor)
            .nth(seat.ordinal)
            .map(|(block, other)| (block, other.rows))?;
        let first = self.row_of_stub_slot(block, 0)?;
        let seated = self.row_of_stub_slot(block, seat.index.min(rows.saturating_sub(1)))?;
        Some(self.settle(seated, false).max(first))
    }

    /// The rendered row of stub `index` of block `block`, if laid out.
    pub(crate) fn row_of_stub_slot(&self, block: usize, index: usize) -> Option<usize> {
        self.layout
            .lines()
            .iter()
            .position(|line| line.stub_slot() == Some((block, index)))
    }

    /// Whether the cursor may rest on `row`: a row of the document, or a
    /// stop in a thread's block, a collapsed stub's row included (ADR
    /// 0076).
    fn is_stop_row(&self, row: usize) -> bool {
        match self.stub_slot_of_row(row) {
            Some((block, index)) => self
                .stubs
                .get(block)
                .is_some_and(|block| block.stops.contains(&index)),
            None => true,
        }
    }

    /// Whether a selection's head may rest on `row`: as [`Self::is_stop_row`],
    /// except that a collapsed stub's rows are no stops, since a
    /// selection never takes one (ADR 0076).
    fn is_selection_stop(&self, row: usize) -> bool {
        match self.stub_slot_of_row(row) {
            Some((block, index)) => self
                .stubs
                .get(block)
                .is_some_and(|block| block.expanded && block.stops.contains(&index)),
            None => true,
        }
    }

    /// Jump to `row`, landing on its first column; a row the cursor may
    /// not rest on settles as a motion would.
    pub(crate) fn goto_row(&mut self, row: usize) {
        self.jump_to_row(row);
    }

    /// Scroll just enough to show `row` with `below` rows under it on
    /// screen, the cursor staying where it is: for the draft's cursor,
    /// which is not the text cursor (ADR 0054), kept above the key bar
    /// that covers the bottom row (ADR 0067).
    pub(crate) fn reveal_row(&mut self, row: usize, below: usize) {
        if row < self.scroll {
            self.scroll = row;
        } else if row + below >= self.scroll + self.height {
            self.scroll = (row + below + 1).saturating_sub(self.height);
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }

    /// The stub block and index row `row` was inserted for, if it is a
    /// stub row.
    pub(crate) fn stub_slot_of_row(&self, row: usize) -> Option<(usize, usize)> {
        self.layout.lines().get(row)?.stub_slot()
    }

    /// Whether `row` is a stub row: a thread's, which no selection takes.
    fn is_stub_row(&self, row: usize) -> bool {
        self.stub_slot_of_row(row).is_some()
    }

    /// The row the cursor settles on when `row` is not a stop: the next
    /// stop when moving forward, else the previous one, the other way
    /// when there is none.
    fn settle(&self, row: usize, forward: bool) -> usize {
        self.settle_on(row, forward, |r| self.is_stop_row(r))
    }

    /// [`Self::settle`] for a selection's head, which skips a collapsed
    /// stub (ADR 0076).
    fn settle_selecting(&self, row: usize, forward: bool) -> usize {
        self.settle_on(row, forward, |r| self.is_selection_stop(r))
    }

    fn settle_on(&self, row: usize, forward: bool, is_stop: impl Fn(usize) -> bool) -> usize {
        if is_stop(row) {
            return row;
        }
        let after = (row + 1..self.layout.lines().len()).find(|&r| is_stop(r));
        let before = (0..row).rev().find(|&r| is_stop(r));
        if forward {
            after.or(before)
        } else {
            before.or(after)
        }
        .unwrap_or(row)
    }

    /// The source line the detached row `row` stands before, if it is one.
    pub(crate) fn detached_anchor_of_row(&self, row: usize) -> Option<usize> {
        self.layout.lines().get(row)?.stands_before()
    }

    /// Move to the detached row standing before source line `anchor`.
    pub(crate) fn goto_detached_row(&mut self, anchor: usize) {
        if let Some(row) = self
            .layout
            .lines()
            .iter()
            .position(|line| line.stands_before() == Some(anchor))
        {
            self.jump_to_row(row);
        }
    }

    pub(crate) fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The document text as laid out.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The text the layout's source ranges index: the diff's target
    /// when that is not the working file, else the text.
    fn shown(&self) -> &str {
        match self.diff().map(|d| &d.body) {
            Some(DiffBody::Diff { target, .. }) => self.text_of(target).unwrap_or(&self.text),
            _ => &self.text,
        }
    }

    /// The text a diff side reads: the view's own copies, or the text
    /// fetched for it; `None` when the copy does not exist.
    fn text_of<'a>(&'a self, text: &'a Text) -> Option<&'a str> {
        match text {
            Text::Working if self.missing.worktree => Some(""),
            Text::Working => Some(&self.text),
            Text::Index => self.index.as_deref(),
            Text::Head => self.head.as_deref(),
            Text::Seen => self.seen.as_deref(),
            Text::Owned(owned) => Some(owned),
        }
    }

    fn source_layout(&self) -> Layout {
        Layout::source_with(
            &self.text,
            self.width,
            &self.syntax.hint,
            &self.syntax.highlighter,
        )
    }

    fn rendered_layout(&self) -> Layout {
        Layout::render_with(&self.text, self.width, &self.syntax.highlighter)
    }

    /// The layout of the home display.
    fn home_layout(&self) -> Layout {
        match self.home() {
            Display::Source => self.source_layout(),
            _ => self.rendered_layout(),
        }
    }

    /// The display the file returns to when a diff closes: rendered for
    /// Markdown, source for anything else.
    fn home(&self) -> Display {
        if self.missing.worktree {
            Display::Source
        } else if self.syntax.markdown {
            Display::Rendered
        } else {
            Display::Source
        }
    }

    /// The diff's sides while one is shown (ADR 0060).
    pub(crate) fn diff(&self) -> Option<&DiffView> {
        self.diff_shown.as_ref().filter(|_| self.diff_view())
    }

    /// The base of the diff shown, if one is.
    #[cfg(test)]
    pub(crate) fn diff_base(&self) -> Option<&Side> {
        self.diff().map(|d| &d.base)
    }

    /// `(added, removed)` between the diff's sides, while one is shown.
    pub(crate) fn pair_counts(&self) -> Option<(usize, usize)> {
        match &self.diff()?.body {
            DiffBody::Diff { base, target } => Some(
                Diff::compare(
                    self.text_of(base)?,
                    self.text_of(target)?,
                    self.compare.whitespace,
                )
                .counts(),
            ),
            DiffBody::Notice(_) => None,
        }
    }

    /// Show `diff` in place of the document.
    pub(crate) fn show_diff(&mut self, diff: DiffView) {
        self.diff_shown = Some(diff);
        self.display = Display::Diff;
        self.relayout();
    }

    /// Leave the diff for the file's home display.
    pub(crate) fn leave_diff(&mut self) {
        if self.display == Display::Diff {
            self.display = self.home();
            self.relayout();
        }
    }

    /// Compare diffs as `compare` says from now on (ADR 0060); an open
    /// diff relays out.
    pub(crate) fn set_compare(&mut self, compare: Compare) {
        if self.compare == compare {
            return;
        }
        self.compare = compare;
        if self.diff_view() {
            self.relayout();
        }
    }

    /// Whether the file has a `HEAD` text to diff against.
    pub(crate) fn has_head(&self) -> bool {
        self.head.is_some()
    }

    /// Whether the file has an index text to diff against.
    pub(crate) fn has_index(&self) -> bool {
        self.index.is_some()
    }

    /// Whether the worktree side of a diff is absent.
    pub(crate) fn worktree_missing(&self) -> bool {
        self.missing.worktree
    }

    /// Whether the index side of a diff is absent.
    pub(crate) fn index_missing(&self) -> bool {
        self.missing.index
    }

    /// Set whether an empty index side represents an absent path.
    pub(crate) fn set_index_missing(&mut self, missing: bool) {
        self.missing.index = missing;
    }

    /// Set whether retained source stands in front of a missing worktree.
    pub(crate) fn set_worktree_missing(&mut self, missing: bool) {
        if self.missing.worktree == missing {
            return;
        }
        self.missing.worktree = missing;
        if missing && self.display != Display::Diff {
            self.display = Display::Source;
        }
        self.rediff();
        if self.diff_view() {
            self.relayout();
        }
    }

    /// Whether the file has a last-seen snapshot to diff against.
    pub(crate) fn has_seen(&self) -> bool {
        self.seen.is_some()
    }

    /// Note that the reader did something here for seen-idle tracking.
    pub(crate) fn touch(&mut self) {
        self.activity = Instant::now();
    }

    /// Time since the reader last did something here.
    pub(crate) fn idle(&self) -> Duration {
        self.activity.elapsed()
    }

    /// Whether 1-based source `line` is within the rows on screen.
    pub(crate) fn line_on_screen(&self, line: usize) -> bool {
        self.layout
            .index()
            .range_of(line)
            .and_then(|range| self.layout.line_at_offset(range.start))
            .is_some_and(|row| row >= self.scroll && row < self.scroll + self.height)
    }

    /// The 1-based line of the first hunk against `HEAD`.
    pub(crate) fn first_hunk_line(&self) -> Option<usize> {
        let diff = self.diff.as_ref()?;
        diff.hunks()
            .first()
            .map(|hunk| hunk.target_line(diff.new_lines()))
    }

    /// The 1-based line of the last hunk against `HEAD`.
    pub(crate) fn last_hunk_line(&self) -> Option<usize> {
        let diff = self.diff.as_ref()?;
        diff.hunks()
            .last()
            .map(|hunk| hunk.target_line(diff.new_lines()))
    }

    pub(crate) fn index(&self) -> &LineIndex {
        self.layout.index()
    }

    pub(crate) fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub(crate) fn scroll(&self) -> usize {
        self.scroll
    }

    pub(crate) fn mode(&self) -> Mode {
        self.mode
    }

    pub(crate) fn input(&self) -> &str {
        &self.input
    }

    pub(crate) fn selection(&self) -> Option<Selection> {
        self.selection
    }

    pub(crate) fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub(crate) fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub(crate) fn changed(&self) -> bool {
        self.changed
    }

    pub(crate) fn source_view(&self) -> bool {
        self.display == Display::Source
    }

    pub(crate) fn diff_view(&self) -> bool {
        self.display == Display::Diff
    }

    /// The gutter status of 1-based source line `line` against `HEAD`.
    pub(crate) fn line_status(&self, line: usize) -> Option<LineStatus> {
        self.diff.as_ref()?.status(line)
    }

    /// Whether 1-based source line `line` is already in the index, so its
    /// change against `HEAD` is staged (ADR 0017). A line the index does
    /// not yet hold shows as unstaged.
    pub(crate) fn line_staged(&self, line: usize) -> bool {
        self.unstaged
            .as_ref()
            .is_some_and(|unstaged| unstaged.status(line).is_none())
    }

    /// `(added, removed)` lines against `HEAD`, `None` outside git.
    pub(crate) fn diff_counts(&self) -> Option<(usize, usize)> {
        self.diff.as_ref().map(Diff::counts)
    }

    /// Whether the aggregate `HEAD -> worktree` comparison has no hunks.
    pub(crate) fn net_diff_empty(&self) -> bool {
        self.diff
            .as_ref()
            .is_some_and(|diff| diff.hunks().is_empty())
    }

    /// Progress through the document as a percentage of rendered lines.
    pub(crate) fn percent(&self) -> usize {
        let last = self.layout.lines().len().saturating_sub(1);
        (self.cursor.row * 100).checked_div(last).unwrap_or(100)
    }

    /// Cursor position in source coordinates: 1-based line and 0-based column.
    pub(crate) fn source_position(&self) -> (usize, usize) {
        let offset = self.cursor_offset().unwrap_or(0);
        let index = self.layout.index();
        (index.line_of(offset), index.column_of(self.shown(), offset))
    }

    /// The source byte offset under the cursor.
    pub(crate) fn cursor_offset(&self) -> Option<usize> {
        self.layout
            .lines()
            .get(self.cursor.row)
            .and_then(|line| line.source_at(self.cursor.col))
    }

    /// Re-lay out for a new pane size, keeping the cursor on the same source.
    pub(crate) fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height.max(1);
        self.relayout();
    }

    /// Update the unobscured viewport without rewrapping or moving visible text.
    pub(crate) fn set_height(&mut self, height: usize) {
        let height = height.max(1);
        if self.height == height {
            return;
        }
        self.height = height;
        self.scroll = self.scroll.min(self.max_scroll());
        if self.cursor.row < self.scroll || self.cursor.row >= self.scroll + height {
            self.ensure_visible();
        }
    }

    /// Replace the document text after a change on disk (ADR 0010 reload).
    pub(crate) fn reload(&mut self, text: String) {
        self.text = text;
        self.missing.worktree = false;
        self.changed = true;
        self.rediff();
        self.relayout();
    }

    /// Set the diff bases, each `None` when it does not exist; the
    /// gutter and the diff view follow.
    pub(crate) fn set_bases(
        &mut self,
        seen: Option<String>,
        index: Option<String>,
        head: Option<String>,
    ) {
        if seen == self.seen && index == self.index && head == self.head {
            return;
        }
        self.seen = seen;
        self.index = index;
        self.head = head;
        self.rediff();
        if self.diff_view() {
            self.relayout();
        }
    }

    fn rediff(&mut self) {
        let worktree = if self.missing.worktree {
            ""
        } else {
            &self.text
        };
        self.diff = self.head.as_deref().map(|base| Diff::new(base, worktree));
        self.unstaged = self.index.as_deref().map(|base| Diff::new(base, worktree));
        if let Some(diff) = &self.diff {
            tracing::debug!(hunks = diff.hunks().len(), "diff against HEAD");
        }
    }

    /// Switch between rendered Markdown and the raw source.
    pub(crate) fn toggle_source_view(&mut self) {
        self.display = match self.display {
            Display::Source => Display::Rendered,
            _ => Display::Source,
        };
        self.relayout();
    }

    /// `]g` within the file: the cursor to the next hunk against `HEAD`.
    /// Reports a wrap instead of taking it, so the app can cross into the
    /// next dirty file (ADR 0017).
    pub(crate) fn next_hunk(&mut self) -> HunkStep {
        self.step_hunk(true)
    }

    /// `[g` within the file: the cursor to the previous hunk.
    pub(crate) fn prev_hunk(&mut self) -> HunkStep {
        self.step_hunk(false)
    }

    fn step_hunk(&mut self, forward: bool) -> HunkStep {
        let Some(diff) = &self.diff else {
            return HunkStep::NoBase;
        };
        if diff.hunks().is_empty() {
            return HunkStep::Clean;
        }
        // Compare rendered rows, not source lines: a hunk on a blank line
        // has no row of its own in the rendered view, so the cursor sits
        // on the row after it and a line comparison would find the same
        // hunk forever.
        let current = self.cursor.row;
        let rows = diff
            .hunks()
            .iter()
            .filter_map(|hunk| self.row_of_source_line(hunk.target_line(diff.new_lines())));
        let found = if forward {
            rows.filter(|row| *row > current).min()
        } else {
            rows.filter(|row| *row < current).max()
        };
        match found {
            Some(row) => {
                self.jump_to_row(row);
                HunkStep::Moved
            }
            None => HunkStep::Wrapped,
        }
    }

    /// The rendered row 1-based source `line` starts on.
    fn row_of_source_line(&self, line: usize) -> Option<usize> {
        let range = self.layout.index().range_of(line)?;
        self.layout.line_at_offset(range.start)
    }

    fn relayout(&mut self) {
        let seat = self.stub_seat();
        self.relayout_seated(seat);
    }

    /// Lay the document out again, the cursor kept on its source line
    /// and column, on its detached row, or on the thread's rows `seat`
    /// names, the stub once the thread has folded.
    fn relayout_seated(&mut self, seat: Option<StubSeat>) {
        // A cursor on a detached row stays on it (ADR 0039).
        let on_detached = self.detached_anchor_of_row(self.cursor.row);
        // A cursor on a row with no source — a blank rendered row — is
        // anchored to the nearest sourced row above it and put back the
        // same number of document rows below that one.
        let anchor_row = if self.cursor_offset().is_none() && on_detached.is_none() {
            (0..self.cursor.row)
                .rev()
                .find(|&row| self.layout.lines()[row].source().is_some())
        } else {
            None
        };
        let rows_below = anchor_row.map_or(0, |above| {
            (above + 1..=self.cursor.row)
                .filter(|&row| !self.is_stub_row(row))
                .count()
        });
        // Anchor by source line and column, not byte offset, so an insertion
        // above the cursor does not drag it onto unrelated text (ADR 0010).
        let (line, column) = match anchor_row {
            Some(above) => {
                let start = self.layout.lines()[above]
                    .source()
                    .map_or(0, |range| range.start);
                let index = self.layout.index();
                (index.line_of(start), index.column_of(self.shown(), start))
            }
            None => self.source_position(),
        };
        // The row that keeps its place on screen: the cursor's, or for a
        // cursor on a stub row, the document row the stub hangs under, so
        // folding the thread under the cursor leaves the view still.
        let kept_row = if self.is_stub_row(self.cursor.row) {
            anchor_row.unwrap_or(self.cursor.row)
        } else {
            self.cursor.row
        };
        let screen_row = kept_row.saturating_sub(self.scroll);
        let layout = match self.display {
            Display::Diff => match self.diff_shown.as_ref().map(|d| &d.body) {
                Some(DiffBody::Diff { base, target }) => {
                    match (self.text_of(base), self.text_of(target)) {
                        (Some(old), Some(new)) => Layout::diff(old, new, self.width, self.compare),
                        // A side that has gone (no `HEAD` after a base
                        // refresh) keeps the display; the layout falls back.
                        _ => self.home_layout(),
                    }
                }
                Some(DiffBody::Notice(text)) => Layout::notice(text, self.width),
                None => Layout::notice("no diff", self.width),
            },
            Display::Source => self.source_layout(),
            Display::Rendered => self.rendered_layout(),
        };
        self.layout = layout.with_rows_before(&self.detached).with_rows_after(
            &self
                .stubs
                .iter()
                .map(|block| (block.anchor, block.rows))
                .collect::<Vec<_>>(),
        );
        let index = self.layout.index();
        let line = line.min(index.line_count());
        let offset = index.offset_at(self.shown(), line, column);
        let row = on_detached
            .and_then(|anchor| {
                self.layout
                    .lines()
                    .iter()
                    .position(|line| line.stands_before() == Some(anchor))
            })
            .or_else(|| offset.and_then(|offset| self.layout.line_at_offset(offset)))
            .unwrap_or(self.cursor.row);
        let mut row = row.min(self.last_row());
        let hangs_under = row;
        for _ in 0..rows_below {
            row = (row + 1..=self.last_row())
                .find(|&r| !self.is_stub_row(r))
                .unwrap_or(row);
        }
        // A cursor on a thread's rows stays on the thread, the row it
        // hangs under keeping its place on screen; else the cursor's
        // row does.
        let kept = if let Some(seated) = seat.and_then(|seat| self.seat_row(seat)) {
            self.cursor.row = seated;
            hangs_under
        } else {
            self.cursor.row = self.settle(row, false);
            self.cursor.row
        };
        self.scroll = kept.saturating_sub(screen_row);
        self.clamp_col();
        self.selection = None;
        self.rescan();
        self.ensure_visible();
    }

    fn last_row(&self) -> usize {
        self.layout.lines().len().saturating_sub(1)
    }

    fn columns(&self, row: usize) -> Vec<usize> {
        self.layout
            .lines()
            .get(row)
            .map(fathomable_core::layout::Line::columns)
            .unwrap_or_default()
    }

    /// Snap the cursor to a grapheme start no further right than `want_col`.
    fn clamp_col(&mut self) {
        let columns = self.columns(self.cursor.row);
        self.cursor.col = columns
            .iter()
            .rev()
            .find(|&&col| col <= self.want_col)
            .copied()
            .unwrap_or(0);
    }

    fn ensure_visible(&mut self) {
        let height = self.height;
        let off = SCROLLOFF.min(height.saturating_sub(1) / 2);
        if self.cursor.row < self.scroll + off {
            self.scroll = self.cursor.row.saturating_sub(off);
        }
        let bottom = self.scroll + height;
        if self.cursor.row + off >= bottom {
            self.scroll = (self.cursor.row + off + 1).saturating_sub(height);
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }

    /// The furthest the viewport scrolls: one row past the last line, so
    /// the end of the file can sit anywhere on screen, as in Helix.
    fn max_scroll(&self) -> usize {
        (self.layout.lines().len() + 1).saturating_sub(self.height)
    }

    fn set_row(&mut self, row: usize) {
        let row = row.min(self.last_row());
        // A row that is no stop is stepped over in the direction of
        // travel (ADR 0049); a selection steps over collapsed stubs.
        let forward = row >= self.cursor.row;
        self.cursor.row = if self.mode == Mode::Select {
            self.settle_selecting(row, forward)
        } else {
            self.settle(row, forward)
        };
        self.clamp_col();
        self.extend_selection();
        self.ensure_visible();
    }

    /// Jump to `row` and land on its first column, as Helix does for
    /// `gg`, `ge`, and `:N` (only `j`/`k` keep the sticky column).
    fn jump_to_row(&mut self, row: usize) {
        self.want_col = 0;
        self.set_row(row);
    }

    fn extend_selection(&mut self) {
        if let Some(selection) = self.selection.as_mut()
            && self.mode == Mode::Select
        {
            selection.head = self.cursor;
        }
    }

    pub(crate) fn move_down(&mut self, n: usize) {
        self.set_row(self.cursor.row.saturating_add(n));
    }

    pub(crate) fn move_up(&mut self, n: usize) {
        self.set_row(self.cursor.row.saturating_sub(n));
    }

    pub(crate) fn half_page_down(&mut self) {
        self.move_down((self.height / 2).max(1));
    }

    pub(crate) fn half_page_up(&mut self) {
        self.move_up((self.height / 2).max(1));
    }

    pub(crate) fn goto_top(&mut self) {
        self.jump_to_row(0);
    }

    pub(crate) fn goto_bottom(&mut self) {
        self.jump_to_row(self.last_row());
    }

    /// The nearest row the cursor may rest on before `row`, if any.
    fn stop_before(&self, row: usize) -> Option<usize> {
        (0..row).rev().find(|&r| self.is_stop_row(r))
    }

    /// The nearest row the cursor may rest on after `row`, if any.
    fn stop_after(&self, row: usize) -> Option<usize> {
        (row + 1..self.layout.lines().len()).find(|&r| self.is_stop_row(r))
    }

    /// One grapheme left; at the first column the cursor wraps onto the
    /// last column of the row above, as Helix does (ADR 0010, amended
    /// 2026-09-04).
    pub(crate) fn move_left(&mut self) {
        let columns = self.columns(self.cursor.row);
        if let Some(&col) = columns.iter().rev().find(|&&c| c < self.cursor.col) {
            self.cursor.col = col;
        } else if let Some(row) = self.stop_before(self.cursor.row) {
            self.cursor.row = row;
            self.cursor.col = self.columns(row).last().copied().unwrap_or(0);
            self.ensure_visible();
        }
        self.want_col = self.cursor.col;
        self.extend_selection();
    }

    /// One grapheme right; at the last column the cursor wraps onto the
    /// first column of the row below.
    pub(crate) fn move_right(&mut self) {
        let columns = self.columns(self.cursor.row);
        if let Some(&col) = columns.iter().find(|&&c| c > self.cursor.col) {
            self.cursor.col = col;
        } else if let Some(row) = self.stop_after(self.cursor.row) {
            self.cursor.row = row;
            self.cursor.col = 0;
            self.ensure_visible();
        }
        self.want_col = self.cursor.col;
        self.extend_selection();
    }

    pub(crate) fn line_start(&mut self) {
        self.cursor.col = 0;
        self.want_col = 0;
        self.extend_selection();
    }

    pub(crate) fn line_end(&mut self) {
        self.cursor.col = self.columns(self.cursor.row).last().copied().unwrap_or(0);
        self.want_col = usize::MAX;
        self.extend_selection();
    }

    /// Scroll the viewport without a cursor jump unless the cursor leaves it.
    pub(crate) fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll());
        let top = self.scroll;
        let bottom = self.scroll + self.height - 1;
        if self.cursor.row < top {
            self.cursor.row = self.settle(top, true);
            self.clamp_col();
        } else if self.cursor.row > bottom {
            self.cursor.row = self.settle(bottom.min(self.last_row()), false);
            self.clamp_col();
        }
    }

    /// Place the cursor at a screen position (mouse click).
    pub(crate) fn click(&mut self, screen_row: usize, col: usize) {
        self.selection = None;
        self.mode = Mode::Normal;
        // A click on a stub row lands on the row it hangs under (ADR 0049,
        // ADR 0073).
        self.cursor.row = self.settle((self.scroll + screen_row).min(self.last_row()), false);
        self.want_col = col;
        self.clamp_col();
    }

    /// Put the cursor on screen row `screen_row` as it is, a collapsed
    /// stub's row or an expanded thread's header included, at its first
    /// column: the mouse's one way onto a row no motion stops on (ADR
    /// 0073, amended 2026-09-09 and 2026-09-10).
    pub(crate) fn rest_on(&mut self, screen_row: usize) {
        self.selection = None;
        self.mode = Mode::Normal;
        self.cursor.row = (self.scroll + screen_row).min(self.last_row());
        self.want_col = 0;
        self.clamp_col();
    }

    /// Extend a mouse selection to a screen position (drag); the head
    /// never rests on a stub row.
    pub(crate) fn drag(&mut self, screen_row: usize, col: usize) {
        let anchor = self.leave_stub();
        let row = self.settle_selecting((self.scroll + screen_row).min(self.last_row()), false);
        let col = self
            .columns(row)
            .iter()
            .rev()
            .find(|&&c| c <= col)
            .copied()
            .unwrap_or(0);
        let selection = self.selection.get_or_insert(Selection {
            anchor,
            head: anchor,
            linewise: false,
        });
        selection.head = Cursor { row, col };
        self.mode = Mode::Select;
    }

    /// Finish a mouse selection: it stays highlighted in select mode so
    /// `y` can copy it or `c` can annotate it (ADR 0013 amends 0010).
    pub(crate) fn release(&mut self) {
        if self.selection.is_some() {
            self.mode = Mode::Select;
        }
    }

    /// A press in the gutter (ADR 0050): select the line under it, whole.
    pub(crate) fn select_line_at(&mut self, screen_row: usize) {
        // A press on a stub row selects the line it hangs under: a
        // selection never takes a stub (ADR 0076).
        self.selection = None;
        self.cursor.row =
            self.settle_selecting((self.scroll + screen_row).min(self.last_row()), false);
        self.want_col = 0;
        self.clamp_col();
        self.selection = Some(Selection {
            anchor: self.cursor,
            head: self.cursor,
            linewise: true,
        });
        self.mode = Mode::Select;
    }

    /// The cursor as a selection may anchor on it: a cursor resting on a
    /// collapsed stub moves to the line the stub hangs under first (ADR
    /// 0076).
    fn leave_stub(&mut self) -> Cursor {
        if !self.is_selection_stop(self.cursor.row) {
            self.cursor.row = self.settle_selecting(self.cursor.row, false);
            self.clamp_col();
        }
        self.cursor
    }

    /// A drag that began in the gutter (ADR 0050): extend by whole lines.
    pub(crate) fn drag_lines(&mut self, screen_row: usize) {
        self.drag(screen_row, 0);
        if let Some(selection) = self.selection.as_mut() {
            selection.linewise = true;
        }
    }

    /// A double-click (ADR 0050): select the word under the pointer, a
    /// run of letters, digits, and underscores, else of other non-blank
    /// characters (vim's `iw`). Blank space selects nothing.
    pub(crate) fn select_word_at(&mut self, screen_row: usize, col: usize) {
        self.click(screen_row, col);
        let row = self.cursor.row;
        let Some((start, end)) = self
            .layout
            .lines()
            .get(row)
            .and_then(|line| word_at(&line.text(), col))
        else {
            return;
        };
        self.cursor.col = end;
        self.want_col = end;
        self.selection = Some(Selection {
            anchor: Cursor { row, col: start },
            head: self.cursor,
            linewise: false,
        });
        self.mode = Mode::Select;
    }

    /// Shift-click (ADR 0050): extend the selection to the pointer, from
    /// the cursor when there is none. A drag does the same.
    pub(crate) fn extend_to(&mut self, screen_row: usize, col: usize) {
        self.drag(screen_row, col);
    }

    /// The URL of the rendered link at `(row, col)`, if the cell is one.
    #[must_use]
    pub(crate) fn link_at(&self, row: usize, col: usize) -> Option<&str> {
        let line = self.layout.lines().get(row)?;
        let mut at = 0;
        for span in line.spans() {
            let width = span.width();
            if col < at + width {
                return match &span.style().face {
                    Face::Link(url) => Some(url),
                    _ => None,
                };
            }
            at += width;
        }
        None
    }

    /// The URL of the link under the cursor, if any.
    #[must_use]
    pub(crate) fn link_at_cursor(&self) -> Option<&str> {
        self.link_at(self.cursor.row, self.cursor.col)
    }

    /// Drop the selection and return to normal mode.
    pub(crate) fn clear_selection(&mut self) {
        self.selection = None;
        if self.mode == Mode::Select {
            self.mode = Mode::Normal;
        }
    }

    /// The 1-based source line rendered row `row` came from, if any.
    pub(crate) fn source_line_of_row(&self, row: usize) -> Option<usize> {
        let line = self.layout.lines().get(row)?;
        let range = line.source()?;
        Some(self.layout.index().line_of(range.start))
    }

    /// The source line under the cursor, or the nearest one above it.
    pub(crate) fn cursor_source_line(&self) -> Option<usize> {
        (0..=self.cursor.row)
            .rev()
            .find_map(|row| self.source_line_of_row(row))
    }

    /// Every source line rendered row `row` came from (a wrapped paragraph
    /// is one row for several lines).
    pub(crate) fn source_lines_of_row(&self, row: usize) -> Option<LineRange> {
        let line = self.layout.lines().get(row)?;
        let range = line.source()?;
        let index = self.layout.index();
        let last = index.line_of(range.end.max(range.start + 1) - 1);
        Some(LineRange::new(index.line_of(range.start), last))
    }

    /// The source lines the selection covers, whichever way it was made.
    pub(crate) fn selected_lines(&self) -> Option<LineRange> {
        let (start, end) = self.selection?.ordered();
        let first = (start.row..=end.row).find_map(|row| self.source_lines_of_row(row))?;
        let last = (start.row..=end.row)
            .rev()
            .find_map(|row| self.source_lines_of_row(row))?;
        Some(LineRange::new(first.start(), last.end()))
    }

    /// `v`: toggle a character selection anchored at the cursor.
    pub(crate) fn select_chars(&mut self) {
        self.toggle_select(false);
    }

    /// `V`: toggle a line selection anchored at the cursor.
    pub(crate) fn select_lines(&mut self) {
        self.toggle_select(true);
    }

    /// `x` (Helix semantics): select the whole cursor line; each further
    /// press takes in one more line below. A `v` selection widens to whole
    /// lines first, keeping its anchor.
    pub(crate) fn extend_line_below(&mut self) {
        if self.mode == Mode::Select
            && let Some(selection) = self.selection.as_mut()
        {
            if selection.linewise {
                self.move_down(1);
            } else {
                selection.linewise = true;
            }
            return;
        }
        let anchor = self.leave_stub();
        self.selection = Some(Selection {
            anchor,
            head: anchor,
            linewise: true,
        });
        self.mode = Mode::Select;
    }

    /// Vim semantics: the same key again leaves select mode, the other key
    /// switches the kind and keeps the anchor.
    fn toggle_select(&mut self, linewise: bool) {
        if self.mode == Mode::Select
            && let Some(selection) = self.selection.as_mut()
        {
            if selection.linewise == linewise {
                self.clear_selection();
            } else {
                selection.linewise = linewise;
            }
            return;
        }
        let anchor = self.leave_stub();
        self.selection = Some(Selection {
            anchor,
            head: anchor,
            linewise,
        });
        self.mode = Mode::Select;
    }

    /// Copy the selection (`y`) and leave select mode. With nothing
    /// selected, the cursor line (ADR 0050).
    pub(crate) fn yank(&mut self) -> Effect {
        let whole_line = self.selection.is_none();
        if whole_line {
            self.selection = Some(Selection {
                anchor: self.cursor,
                head: self.cursor,
                linewise: true,
            });
        }
        let effect = self.copy_selection();
        if whole_line {
            self.selection = None;
        }
        self.mode = Mode::Normal;
        effect
    }

    fn copy_selection(&mut self) -> Effect {
        let Some(text) = self.selected_source() else {
            return Effect::None;
        };
        let lines = text.lines().count().max(1);
        self.message = Some(format!(
            "copied {lines} line{}",
            if lines == 1 { "" } else { "s" }
        ));
        Effect::Copy(text)
    }

    /// The source Markdown behind the selection, if any.
    pub(crate) fn selected_source(&self) -> Option<String> {
        let selection = self.selection?;
        let (start, end) = selection.ordered();
        let lines = self.layout.lines();
        let (from, to) = if selection.linewise {
            // Whole source lines, including list markers and other syntax
            // the rendered spans do not cover.
            let index = self.layout.index();
            let from = lines[start.row..=end.row]
                .iter()
                .find_map(fathomable_core::layout::Line::source)
                .and_then(|range| index.range_of(index.line_of(range.start)))
                .map(|range| range.start)?;
            let to = lines[start.row..=end.row]
                .iter()
                .rev()
                .find_map(fathomable_core::layout::Line::source)
                .and_then(|range| index.range_of(index.line_of(range.end.max(range.start + 1) - 1)))
                .map(|range| range.end)?;
            (from, to)
        } else {
            let from = lines.get(start.row)?.source_at(start.col)?;
            let last = lines.get(end.row)?.source_at(end.col)?;
            let to = last
                + self
                    .text
                    .get(last..)
                    .and_then(|rest| rest.chars().next())
                    .map_or(0, char::len_utf8);
            (from, to)
        };
        if from >= to {
            return None;
        }
        let shown = self.shown();
        let from = floor_char(shown, from);
        let to = ceil_char(shown, to);
        shown.get(from..to).map(str::to_owned)
    }

    /// Esc: clear input, pending keys, selection, then search highlights.
    /// `Esc`: clear the input, else the selection, else the search
    /// highlight; whether there was one to clear, so the app can take
    /// the next step of the cascade (leaving a diff, ADR 0060).
    pub(crate) fn escape(&mut self) -> bool {
        self.message = None;
        if matches!(self.mode, Mode::Command | Mode::Search { .. }) {
            self.mode = Mode::Normal;
            self.input.clear();
        } else if self.selection.is_some() {
            self.selection = None;
            self.mode = Mode::Normal;
        } else if self.pattern.is_some() {
            self.clear_highlight();
        } else {
            return false;
        }
        true
    }

    pub(crate) fn clear_highlight(&mut self) {
        self.pattern = None;
        self.matches.clear();
    }

    pub(crate) fn clear_message(&mut self) {
        self.message = None;
    }

    pub(crate) fn start_command(&mut self) {
        self.mode = Mode::Command;
        self.input.clear();
        self.message = None;
    }

    pub(crate) fn start_search(&mut self, backward: bool) {
        self.mode = Mode::Search { backward };
        self.backward = backward;
        self.input.clear();
        self.message = None;
    }

    /// Type into the `:` or `/` line; searches update incrementally.
    pub(crate) fn input_char(&mut self, ch: char) {
        self.input.push(ch);
        self.incremental();
    }

    pub(crate) fn input_backspace(&mut self) {
        self.input.pop();
        self.incremental();
    }

    fn incremental(&mut self) {
        if let Mode::Search { .. } = self.mode {
            match compile(&self.input) {
                Ok(pattern) => {
                    self.pattern = Some(pattern);
                    self.rescan();
                    self.message = None;
                    self.jump_from(self.cursor, self.backward, false);
                }
                Err(error) => {
                    self.matches.clear();
                    self.message = Some(error);
                }
            }
        }
    }

    /// Enter on the input line.
    pub(crate) fn confirm(&mut self) -> Effect {
        match self.mode {
            Mode::Command => {
                let command = std::mem::take(&mut self.input);
                self.mode = Mode::Normal;
                self.execute(command.trim())
            }
            Mode::Search { .. } => {
                self.mode = Mode::Normal;
                if self.input.is_empty() {
                    self.search_next(false);
                } else {
                    self.incremental();
                    self.input.clear();
                }
                Effect::None
            }
            _ => Effect::None,
        }
    }

    fn execute(&mut self, command: &str) -> Effect {
        match command {
            "q" | "q!" | "quit" => Effect::Quit,
            "noh" | "nohlsearch" => {
                self.clear_highlight();
                Effect::None
            }
            "source" => {
                self.toggle_source_view();
                Effect::None
            }
            "" => Effect::None,
            number if number.chars().all(|c| c.is_ascii_digit()) => {
                if let Ok(line) = number.parse::<usize>() {
                    self.goto_source_line(line);
                }
                Effect::None
            }
            // Anything else is the app's to run or refuse.
            other => Effect::Command(other.to_owned()),
        }
    }

    /// Move to the rendered line showing source line `line`.
    pub(crate) fn goto_source_line(&mut self, line: usize) {
        if let Some(row) = self.row_of_source_line(line) {
            self.jump_to_row(row);
        }
    }

    /// Reveal a source range without selecting it.
    ///
    /// The cursor lands on `start`, and the view scrolls so `end` is also
    /// visible when the range fits.
    pub(crate) fn reveal_source_range(&mut self, start: usize, end: usize) {
        self.goto_source_line(start);
        let Some(last) = self
            .row_of_source_line(end)
            .filter(|row| *row > self.cursor.row)
        else {
            return;
        };
        let bottom = self.scroll + self.height;
        if last >= bottom {
            let wanted = (last + 1).saturating_sub(self.height);
            self.scroll = wanted.min(self.cursor.row).min(self.max_scroll());
        }
    }

    /// `n` / `N`: next match in the search direction, flipped by `reverse`.
    pub(crate) fn search_next(&mut self, reverse: bool) {
        if self.pattern.is_none() {
            self.message = Some("no previous search".to_owned());
            return;
        }
        let backward = self.backward ^ reverse;
        self.jump_from(self.cursor, backward, true);
    }

    fn rescan(&mut self) {
        self.matches.clear();
        let Some(pattern) = &self.pattern else {
            return;
        };
        for (row, line) in self.layout.lines().iter().enumerate() {
            let text = line.text();
            for found in pattern.find_iter(&text) {
                let start = display_width(&text[..found.start()]);
                let end = start + display_width(found.as_str()).max(1);
                self.matches.push(Match { row, start, end });
            }
        }
    }

    /// Jump to the match after (or before) `from`; `skip_current` excludes a
    /// match sitting exactly on the cursor, as `n` must.
    fn jump_from(&mut self, from: Cursor, backward: bool, skip_current: bool) {
        if self.matches.is_empty() {
            if self.pattern.is_some() {
                self.message = Some(format!("pattern not found: {}", self.input));
            }
            return;
        }
        let at = |m: &Match| Cursor {
            row: m.row,
            col: m.start,
        };
        let target = if backward {
            let before = self.matches.iter().rev().find(|m| at(m) < from);
            before.or_else(|| {
                self.message = Some("search hit TOP, continuing at BOTTOM".to_owned());
                self.matches.last()
            })
        } else {
            let after = self.matches.iter().find(|m| {
                if skip_current {
                    at(m) > from
                } else {
                    at(m) >= from
                }
            });
            after.or_else(|| {
                self.message = Some("search hit BOTTOM, continuing at TOP".to_owned());
                self.matches.first()
            })
        };
        if let Some(target) = target.copied() {
            self.cursor = at(&target);
            self.want_col = self.cursor.col;
            self.ensure_visible();
        }
    }
}

/// Compile a search pattern with smart case (ADR 0010).
fn compile(input: &str) -> Result<Regex, String> {
    let smart = if input.chars().any(char::is_uppercase) {
        String::new()
    } else {
        "(?i)".to_owned()
    };
    Regex::new(&format!("{smart}{input}")).map_err(|error| match error {
        regex::Error::Syntax(msg) => msg.lines().last().unwrap_or("bad pattern").to_owned(),
        other => other.to_string(),
    })
}

/// The display columns a word covers at `col`: its first column and the
/// column of its last character. Letters, digits, and underscores are one
/// class, other non-blank characters another; blanks are no word.
fn word_at(text: &str, col: usize) -> Option<(usize, usize)> {
    #[derive(PartialEq, Eq, Clone, Copy)]
    enum Class {
        Word,
        Other,
        Blank,
    }
    let class = |ch: char| {
        if ch.is_alphanumeric() || ch == '_' {
            Class::Word
        } else if ch.is_whitespace() {
            Class::Blank
        } else {
            Class::Other
        }
    };
    let mut cells = Vec::new();
    let mut at = 0;
    for ch in text.chars() {
        cells.push((at, class(ch)));
        at += display_width(ch.encode_utf8(&mut [0; 4]));
    }
    let index = cells.iter().rposition(|&(start, _)| start <= col)?;
    let kind = cells[index].1;
    if kind == Class::Blank {
        return None;
    }
    let first = (0..index)
        .rev()
        .take_while(|&i| cells[i].1 == kind)
        .last()
        .unwrap_or(index);
    let last = (index..cells.len())
        .take_while(|&i| cells[i].1 == kind)
        .last()
        .unwrap_or(index);
    Some((cells[first].0, cells[last].0))
}

fn floor_char(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn ceil_char(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use fathomable_core::annotations::LineRange;

    use super::{Cursor, DiffBody, DiffView, Effect, HunkStep, Mode, Side, Text, View};

    const DOC: &str = "# Title\n\nalpha beta\n\n- one\n- two\n- three\n\nlast *word* here\n";

    fn view() -> View {
        View::new(DOC.to_owned(), 40, 5)
    }

    #[test]
    fn cursor_moves_by_rendered_line_and_keeps_scrolloff() {
        let mut v = view();
        assert_eq!(v.layout().lines().len(), 9);
        v.move_down(3);
        assert_eq!(v.cursor().row, 3);
        // With height 5 and scrolloff 2 (half of height-1), row 3 scrolls.
        assert_eq!(v.scroll(), 1);
        v.goto_bottom();
        assert_eq!(v.cursor().row, 8);
        // Scrolloff pushes the view one row past the end.
        assert_eq!(v.scroll(), 5);
        v.goto_top();
        assert_eq!(v.scroll(), 0);
        v.half_page_down();
        assert_eq!(v.cursor().row, 2);
    }

    #[test]
    fn horizontal_motion_steps_graphemes_and_remembers_column() {
        let mut v = View::new("日本語\n\nab\n\n日本語\n".to_owned(), 40, 10);
        v.move_right();
        v.move_right();
        assert_eq!(v.cursor().col, 4);
        v.move_down(2);
        assert_eq!(v.cursor().col, 1, "clamped to a grapheme start on 'ab'");
        v.move_down(2);
        assert_eq!(v.cursor().col, 4, "wanted column restored");
        v.line_end();
        v.move_left();
        assert_eq!(v.cursor().col, 2);
    }

    #[test]
    fn source_position_is_in_source_coordinates() {
        let mut v = view();
        v.move_down(2);
        v.move_right();
        assert_eq!(v.source_position(), (3, 1));
        v.goto_bottom();
        assert_eq!(v.source_position().0, 9);
    }

    #[test]
    fn reload_reanchors_cursor_to_same_source_line() {
        let mut v = view();
        v.move_down(4);
        assert_eq!(v.source_position().0, 5);
        let screen_row = v.cursor().row - v.scroll();
        assert_eq!(screen_row, 2, "cursor sits mid-screen before the reload");
        let text = "# Title\n\nnew paragraph inserted\n\nalpha beta\n\n- one\n- two\n- three\n\nlast *word* here\n";
        v.reload(text.to_owned());
        assert!(v.changed());
        // ADR 0010: anchor by source line number, so line 5 is now "alpha beta".
        assert_eq!(v.source_position().0, 5);
        assert_eq!(v.layout().lines()[v.cursor().row].text(), "alpha beta");
        assert_eq!(v.cursor().row - v.scroll(), screen_row);
        // Deleted line: nearest surviving line wins.
        v.reload("# Title\n\nalpha\n".to_owned());
        assert_eq!(v.cursor().row, 2);
    }

    #[test]
    fn horizontal_motion_wraps_across_rows() {
        let mut v = view();
        // Row 2 is "alpha beta"; row 1 is blank, row 3 is blank.
        v.move_down(2);
        v.move_left();
        assert_eq!(
            (v.cursor().row, v.cursor().col),
            (1, 0),
            "h wraps onto the blank row above"
        );
        v.move_left();
        assert_eq!(v.cursor().row, 0);
        assert_eq!(
            v.cursor().col,
            v.columns(0).last().copied().unwrap_or(0),
            "onto the last column"
        );
        v.move_left();
        assert_eq!(
            v.cursor().col,
            v.columns(0).last().copied().unwrap_or(0) - 1
        );
        v.goto_top();
        v.move_left();
        assert_eq!(
            (v.cursor().row, v.cursor().col),
            (0, 0),
            "nothing above the first row"
        );
        v.move_down(2);
        v.line_end();
        v.move_right();
        assert_eq!(
            (v.cursor().row, v.cursor().col),
            (3, 0),
            "l wraps onto the row below"
        );
        v.move_right();
        assert_eq!((v.cursor().row, v.cursor().col), (4, 0));
        v.goto_bottom();
        v.line_end();
        let end = v.cursor().col;
        v.move_right();
        assert_eq!(
            (v.cursor().row, v.cursor().col),
            (8, end),
            "nothing below the last row"
        );
    }

    #[test]
    fn selection_grows_across_a_wrap() {
        let mut v = view();
        // Row 2 is "alpha beta"; select from its last word across the
        // blank row into "- one".
        v.move_down(2);
        v.line_end();
        v.move_left();
        v.move_left();
        v.move_left();
        v.select_chars();
        for _ in 0..9 {
            v.move_right();
        }
        assert_eq!(v.cursor().row, 4);
        assert_eq!(v.yank(), Effect::Copy("beta\n\n- one".to_owned()));
    }

    #[test]
    fn line_selection_copies_source_markdown() {
        let mut v = view();
        v.move_down(4);
        v.select_lines();
        assert_eq!(v.mode(), Mode::Select);
        v.move_down(1);
        assert_eq!(v.yank(), Effect::Copy("- one\n- two".to_owned()));
        assert_eq!(v.mode(), Mode::Normal);
        assert_eq!(v.message(), Some("copied 2 lines"));
    }

    #[test]
    fn char_selection_toggles_and_switches_kind_like_vim() {
        let mut v = view();
        // Row 2 is "alpha beta"; select "lpha" one character at a time.
        v.move_down(2);
        v.move_right();
        v.select_chars();
        assert_eq!(v.mode(), Mode::Select);
        assert!(v.selection().is_some_and(|s| !s.linewise));
        v.move_right();
        v.move_right();
        v.move_right();
        assert_eq!(v.yank(), Effect::Copy("lpha".to_owned()));
        // `v` again leaves select mode; `V` inside `v` switches to lines.
        v.select_chars();
        v.select_chars();
        assert_eq!(v.mode(), Mode::Normal);
        assert!(v.selection().is_none());
        v.select_chars();
        v.select_lines();
        assert!(v.selection().is_some_and(|s| s.linewise));
        v.select_lines();
        assert_eq!(v.mode(), Mode::Normal);
    }

    #[test]
    fn extend_line_below_grows_one_line_per_press_like_helix() {
        let mut v = view();
        // Row 4 is "- one"; first `x` selects just that line.
        v.move_down(4);
        v.extend_line_below();
        assert_eq!(v.mode(), Mode::Select);
        assert!(v.selection().is_some_and(|s| s.linewise));
        assert_eq!(v.selected_lines(), Some(LineRange::new(5, 5)));
        // Each further press takes in the next line down.
        v.extend_line_below();
        assert_eq!(v.selected_lines(), Some(LineRange::new(5, 6)));
        v.extend_line_below();
        assert_eq!(v.selected_lines(), Some(LineRange::new(5, 7)));
        // A `v` selection widens to whole lines before growing.
        v.escape();
        v.goto_top();
        v.move_down(4);
        v.move_right();
        v.select_chars();
        v.extend_line_below();
        assert!(v.selection().is_some_and(|s| s.linewise));
        assert_eq!(v.selected_lines(), Some(LineRange::new(5, 5)));
        v.extend_line_below();
        assert_eq!(v.selected_lines(), Some(LineRange::new(5, 6)));
    }

    #[test]
    fn mouse_drag_stays_selected_until_yanked() {
        let mut v = view();
        // Click on "beta" (row 2, col 6) then drag to the end of "one".
        v.click(2, 6);
        v.drag(4, 4);
        v.release();
        assert_eq!(v.mode(), Mode::Select, "release does not copy (ADR 0013)");
        assert_eq!(v.selected_lines(), Some(LineRange::new(3, 5)));
        assert_eq!(v.yank(), Effect::Copy("beta\n\n- one".to_owned()));
        assert_eq!(v.mode(), Mode::Normal);
        // Dragging inside emphasis maps back to the raw markup.
        v.click(8, 5);
        v.drag(8, 8);
        v.release();
        assert_eq!(v.yank(), Effect::Copy("word".to_owned()));
    }

    #[test]
    fn source_lines_are_found_from_rendered_rows() {
        let mut v = view();
        assert_eq!(v.source_line_of_row(0), Some(1));
        assert_eq!(v.cursor_source_line(), Some(1));
        v.move_down(1);
        assert_eq!(
            v.cursor_source_line(),
            Some(1),
            "blank row falls back upward"
        );
        v.select_lines();
        v.move_down(3);
        assert_eq!(v.selected_lines(), Some(LineRange::new(3, 5)));
        v.clear_selection();
        assert_eq!(v.mode(), Mode::Normal);
        assert!(v.selection().is_none());
    }

    #[test]
    fn search_is_incremental_smart_case_and_wraps() {
        let mut v = view();
        v.start_search(false);
        for ch in "T".chars() {
            v.input_char(ch);
        }
        assert_eq!(v.matches().len(), 1, "uppercase pattern is case-sensitive");
        v.escape();
        v.start_search(false);
        for ch in "t".chars() {
            v.input_char(ch);
        }
        assert!(v.matches().len() > 3, "lowercase pattern ignores case");
        assert_eq!(v.confirm(), Effect::None);
        assert_eq!(v.mode(), Mode::Normal);
        v.goto_bottom();
        v.line_end();
        v.search_next(false);
        assert_eq!(v.cursor(), Cursor { row: 0, col: 0 });
        assert_eq!(v.message(), Some("search hit BOTTOM, continuing at TOP"));
        v.search_next(true);
        assert_eq!(v.message(), Some("search hit TOP, continuing at BOTTOM"));
    }

    #[test]
    fn bad_regex_reports_and_stays_put() {
        let mut v = view();
        v.move_down(2);
        v.start_search(false);
        v.input_char('(');
        assert!(v.message().is_some());
        assert_eq!(v.cursor().row, 2);
        assert!(v.matches().is_empty());
    }

    #[test]
    fn commands_quit_goto_and_toggle_source() {
        let mut v = view();
        v.move_down(2);
        v.move_right();
        v.move_right();
        assert_eq!(v.cursor().col, 2);
        v.start_command();
        for ch in "9".chars() {
            v.input_char(ch);
        }
        assert_eq!(v.confirm(), Effect::None);
        assert_eq!(v.source_position().0, 9);
        assert_eq!(v.cursor().col, 0, ":N lands on the first column");
        v.start_command();
        v.input_char('q');
        assert_eq!(v.confirm(), Effect::Quit);
        v.start_command();
        for ch in "source".chars() {
            v.input_char(ch);
        }
        v.confirm();
        assert!(v.source_view());
        assert_eq!(v.layout().lines()[0].text(), "# Title");
        assert_eq!(v.source_position().0, 9, "toggle keeps the source line");
        v.move_right();
        v.goto_top();
        assert_eq!(v.cursor().col, 0, "gg lands on the first column");
    }

    #[test]
    fn backspace_on_empty_input_stays_in_command_and_search() {
        let mut v = view();
        v.start_command();
        v.input_char('q');
        v.input_backspace();
        v.input_backspace();
        assert_eq!(v.mode(), Mode::Command);
        assert!(v.input().is_empty());
        v.escape();
        assert_eq!(v.mode(), Mode::Normal);

        v.start_search(false);
        v.input_backspace();
        assert!(matches!(v.mode(), Mode::Search { .. }));
    }

    #[test]
    fn escape_clears_in_order_and_never_quits() {
        let mut v = view();
        v.start_search(false);
        v.input_char('a');
        v.confirm();
        assert!(!v.matches().is_empty());
        v.select_lines();
        v.escape();
        assert!(v.selection().is_none());
        assert!(!v.matches().is_empty());
        v.escape();
        assert!(v.matches().is_empty());
        v.escape();
        assert_eq!(v.mode(), Mode::Normal);
    }

    #[test]
    fn diff_base_drives_gutter_status_hunk_jumps_and_the_diff_view() {
        use fathomable_core::diff::LineStatus;

        let mut v = view();
        assert_eq!(v.diff_counts(), None);
        assert_eq!(v.line_status(1), None);
        assert_eq!(v.next_hunk(), HunkStep::NoBase);

        // The committed text lacked "- two" and had a different last line;
        // the index already holds "- two", so that hunk is staged.
        let head = "# Title\n\nalpha beta\n\n- one\n- three\n\nlast word here\n".to_owned();
        let index = "# Title\n\nalpha beta\n\n- one\n- two\n- three\n\nlast word here\n".to_owned();
        v.set_bases(None, Some(index), Some(head));
        assert_eq!(v.line_status(6), Some(LineStatus::Added));
        assert!(v.line_staged(6), "the index has the added line");
        assert_eq!(v.line_status(9), Some(LineStatus::Modified));
        assert!(!v.line_staged(9), "the last line is not staged");
        assert_eq!(v.line_status(1), None);
        assert_eq!(v.diff_counts(), Some((2, 1)));
        assert_eq!(v.first_hunk_line(), Some(6));
        assert_eq!(v.last_hunk_line(), Some(9));

        assert_eq!(v.next_hunk(), HunkStep::Moved);
        assert_eq!(v.source_position().0, 6);
        assert_eq!(v.next_hunk(), HunkStep::Moved);
        assert_eq!(v.source_position().0, 9);
        assert_eq!(v.next_hunk(), HunkStep::Wrapped);
        assert_eq!(v.source_position().0, 9, "a wrap is reported, not taken");
        assert_eq!(v.prev_hunk(), HunkStep::Moved);
        assert_eq!(v.source_position().0, 6);
        assert_eq!(v.prev_hunk(), HunkStep::Wrapped);
        v.goto_source_line(9);

        let diff = DiffView {
            base: Side::Head,
            target: Side::Working,
            header: String::new(),
            badge: String::new(),
            body: DiffBody::Diff {
                base: Text::Head,
                target: Text::Working,
            },
        };
        v.show_diff(diff.clone());
        assert!(v.diff_view());
        assert_eq!(v.source_position().0, 9, "relayout keeps the source line");
        let texts: Vec<String> = v
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .collect();
        assert!(texts.iter().any(|t| t == "+- two"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "-last word here"), "{texts:?}");
        v.start_command();
        for ch in "diff".chars() {
            v.input_char(ch);
        }
        assert!(
            matches!(v.confirm(), Effect::Command(c) if c == "diff"),
            ":diff is the app's to run"
        );
        v.leave_diff();
        assert!(!v.diff_view());

        // A reload against the same base re-diffs; an identical text is clean.
        v.reload("# Title\n\nalpha beta\n\n- one\n- three\n\nlast word here\n".to_owned());
        assert_eq!(v.diff_counts(), Some((0, 0)));
        assert_eq!(v.next_hunk(), HunkStep::Clean);
        v.show_diff(diff);
        assert_eq!(v.layout().lines().len(), 1);
        v.set_bases(None, None, None);
        assert_eq!(v.diff_counts(), None);
        assert!(v.diff_view(), "display sticks; layout falls back");
        assert!(v.layout().lines().len() > 1);
    }

    #[test]
    fn app_commands_are_forwarded_and_activity_is_tracked() {
        let mut v = view();
        v.start_command();
        for ch in "diff seen".chars() {
            v.input_char(ch);
        }
        assert!(matches!(v.confirm(), Effect::Command(c) if c == "diff seen"));
        v.start_command();
        for ch in "follow".chars() {
            v.input_char(ch);
        }
        assert!(matches!(v.confirm(), Effect::Command(c) if c == "follow"));
        assert!(v.line_on_screen(1));
        assert!(!v.line_on_screen(usize::MAX));
        v.touch();
        assert!(v.idle() < Duration::from_secs(1));
    }

    #[test]
    fn wheel_scroll_drags_cursor_along() {
        let mut v = view();
        v.scroll_by(3);
        assert_eq!(v.scroll(), 3);
        assert_eq!(v.cursor().row, 3);
        v.scroll_by(-10);
        assert_eq!(v.scroll(), 0);
    }
}
