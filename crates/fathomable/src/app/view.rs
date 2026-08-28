// @okf-doc: /decisions/0010-viewer-ux.md
//! Viewer state: cursor, scrolling, selection, search, and reload anchoring.
//!
//! Everything here is plain data so the ADR 0010 conventions can be tested
//! without a terminal; `ui` draws it and `keys` drives it.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fathomable_core::annotations::LineRange;
use fathomable_core::diff::{Diff, LineStatus};
use fathomable_core::highlight::Highlighter;
use fathomable_core::layout::{Layout, LineIndex, display_width};
use regex::Regex;

use super::hscroll::HScroll;

/// Rendered lines kept visible above and below the cursor.
const SCROLLOFF: usize = 3;

/// Editing-style mode shown in the status pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
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
pub enum Display {
    /// Rendered Markdown.
    #[default]
    Rendered,
    /// The raw source (`gs`, ADR 0010).
    Source,
    /// A unified diff against `HEAD` (`gd`, ADR 0006, ADR 0017).
    Diff,
    /// A unified diff against the last-seen snapshot (ADR 0015).
    DiffSeen,
}

/// A position in rendered coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
}

/// A selection between an anchor and a moving head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: Cursor,
    pub head: Cursor,
    /// Whole lines (`V`) rather than columns (mouse drag).
    pub linewise: bool,
}

impl Selection {
    /// The selection ordered top-left to bottom-right.
    pub fn ordered(&self) -> (Cursor, Cursor) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Whether rendered cell `(row, col)` is inside the selection.
    pub fn contains(&self, row: usize, col: usize) -> bool {
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
pub struct Match {
    pub row: usize,
    pub start: usize,
    pub end: usize,
}

/// What the event loop must do after handling input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    None,
    Quit,
    Copy(String),
    /// A `:` command the app handles (`:follow ...`, ADR 0015).
    Command(String),
    /// Hand the comment draft to `$EDITOR` (ADR 0018).
    EditDraft,
}

/// What `]g` / `[g` did (ADR 0017), so the app can cross into the next
/// dirty file when the hunks of this one run out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HunkStep {
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
pub struct Syntax {
    /// The shared highlighter.
    pub highlighter: Arc<Highlighter>,
    /// Language hint for the source layout: the file extension, or empty.
    pub hint: String,
    /// Whether the file opens rendered as Markdown rather than as source.
    pub markdown: bool,
}

impl Syntax {
    /// Plain Markdown: no colours, rendered first.
    #[must_use]
    pub fn plain() -> Self {
        Self {
            highlighter: Arc::new(Highlighter::plain()),
            hint: String::new(),
            markdown: true,
        }
    }
}

#[derive(Debug)]
pub struct View {
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
    pending: Option<char>,
    /// The column offset of unwrapped lines (ADR 0029).
    hscroll: HScroll,
    /// Digits typed before a key, zero for none.
    count: usize,
}

impl View {
    /// Lay `text` out for a text area of `width` by `height` cells, as
    /// uncoloured Markdown.
    pub fn new(text: String, width: usize, height: usize) -> Self {
        Self::with_syntax(text, width, height, Syntax::plain())
    }

    /// Lay `text` out for a text area of `width` by `height` cells,
    /// coloured and initially displayed as `syntax` says.
    pub fn with_syntax(text: String, width: usize, height: usize, syntax: Syntax) -> Self {
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
            pending: None,
            hscroll: HScroll::default(),
            count: 0,
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// The document text as laid out.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Note that the reader did something here (ADR 0015 guardrails and
    /// seen-idle).
    pub fn touch(&mut self) {
        self.activity = Instant::now();
    }

    /// Time since the reader last did something here.
    pub fn idle(&self) -> Duration {
        self.activity.elapsed()
    }

    /// Whether the diff view is showing the last-seen diff rather than
    /// the `HEAD` one.
    pub fn diff_seen(&self) -> bool {
        self.display == Display::DiffSeen
    }

    /// Whether 1-based source `line` is within the rows on screen.
    pub fn line_on_screen(&self, line: usize) -> bool {
        self.layout
            .index()
            .range_of(line)
            .and_then(|range| self.layout.line_at_offset(range.start))
            .is_some_and(|row| row >= self.scroll && row < self.scroll + self.height)
    }

    /// The 1-based line of the first hunk against `HEAD`.
    pub fn first_hunk_line(&self) -> Option<usize> {
        let diff = self.diff.as_ref()?;
        diff.hunks()
            .first()
            .map(|hunk| hunk.target_line(diff.new_lines()))
    }

    /// The 1-based line of the last hunk against `HEAD`.
    pub fn last_hunk_line(&self) -> Option<usize> {
        let diff = self.diff.as_ref()?;
        diff.hunks()
            .last()
            .map(|hunk| hunk.target_line(diff.new_lines()))
    }

    pub fn index(&self) -> &LineIndex {
        self.layout.index()
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn pending(&self) -> Option<char> {
        self.pending
    }

    /// The count typed so far, zero for none.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Append a digit to the count; it saturates rather than overflows.
    pub fn push_count(&mut self, digit: u32) {
        self.count = self
            .count
            .saturating_mul(10)
            .saturating_add(usize::try_from(digit).unwrap_or(0));
    }

    /// Take the count for the key it applies to, zero for none.
    pub fn take_count(&mut self) -> usize {
        std::mem::take(&mut self.count)
    }

    /// How many columns the unwrapped lines are scrolled by (ADR 0029).
    pub fn column_offset(&self) -> usize {
        self.hscroll.offset(&self.layout)
    }

    /// `zl` and friends: scroll the unwrapped lines by `delta` columns,
    /// clamped to the widest of them.
    pub fn scroll_columns(&mut self, delta: isize) {
        self.hscroll.scroll_by(delta, &self.layout);
    }

    /// The screen column of line column `col` on rendered `row`, or
    /// `None` when the horizontal scroll has moved it out of view.
    pub fn screen_col(&self, row: usize, col: usize) -> Option<usize> {
        let Some(line) = self.layout.lines().get(row) else {
            return Some(col);
        };
        if !line.is_unwrapped() || col < line.fixed_cells() {
            return Some(col);
        }
        (col - line.fixed_cells())
            .checked_sub(self.column_offset())
            .map(|shifted| shifted + line.fixed_cells())
    }

    /// The line column shown at screen column `col` on rendered `row`.
    fn line_col(&self, row: usize, col: usize) -> usize {
        match self.layout.lines().get(row) {
            Some(line) if line.is_unwrapped() && col >= line.fixed_cells() => {
                col + self.column_offset()
            }
            _ => col,
        }
    }

    pub fn changed(&self) -> bool {
        self.changed
    }

    pub fn source_view(&self) -> bool {
        self.display == Display::Source
    }

    pub fn diff_view(&self) -> bool {
        matches!(self.display, Display::Diff | Display::DiffSeen)
    }

    /// The gutter status of 1-based source line `line` against `HEAD`.
    pub fn line_status(&self, line: usize) -> Option<LineStatus> {
        self.diff.as_ref()?.status(line)
    }

    /// Whether 1-based source line `line` is already in the index, so its
    /// change against `HEAD` is staged (ADR 0017). A line the index does
    /// not yet hold shows as unstaged.
    pub fn line_staged(&self, line: usize) -> bool {
        self.unstaged
            .as_ref()
            .is_some_and(|unstaged| unstaged.status(line).is_none())
    }

    /// `(added, removed)` lines against `HEAD`, `None` outside git.
    pub fn diff_counts(&self) -> Option<(usize, usize)> {
        self.diff.as_ref().map(Diff::counts)
    }

    /// Progress through the document as a percentage of rendered lines.
    pub fn percent(&self) -> usize {
        let last = self.layout.lines().len().saturating_sub(1);
        (self.cursor.row * 100).checked_div(last).unwrap_or(100)
    }

    /// Cursor position in source coordinates: 1-based line and 0-based column.
    pub fn source_position(&self) -> (usize, usize) {
        let offset = self.cursor_offset().unwrap_or(0);
        let index = self.layout.index();
        (index.line_of(offset), index.column_of(&self.text, offset))
    }

    /// The source byte offset under the cursor.
    pub fn cursor_offset(&self) -> Option<usize> {
        self.layout
            .lines()
            .get(self.cursor.row)
            .and_then(|line| line.source_at(self.cursor.col))
    }

    /// Re-lay out for a new pane size, keeping the cursor on the same source.
    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height.max(1);
        self.relayout();
    }

    /// Replace the document text after a change on disk (ADR 0010 reload).
    pub fn reload(&mut self, text: String) {
        self.text = text;
        self.changed = true;
        self.rediff();
        self.relayout();
    }

    /// Set the diff bases, each `None` when it does not exist; the
    /// gutter and the diff view follow.
    pub fn set_bases(&mut self, seen: Option<String>, index: Option<String>, head: Option<String>) {
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
        self.diff = self.head.as_deref().map(|base| Diff::new(base, &self.text));
        self.unstaged = self
            .index
            .as_deref()
            .map(|base| Diff::new(base, &self.text));
        if let Some(diff) = &self.diff {
            tracing::debug!(hunks = diff.hunks().len(), "diff against HEAD");
        }
    }

    /// Switch between rendered Markdown and the raw source.
    pub fn toggle_source_view(&mut self) {
        self.display = match self.display {
            Display::Source => Display::Rendered,
            _ => Display::Source,
        };
        self.relayout();
    }

    /// `gd` / `:diff`: the unified diff against `HEAD`, or back to the
    /// rendered view (ADR 0017).
    pub fn toggle_diff_view(&mut self) {
        if self.display == Display::Diff {
            self.display = Display::Rendered;
        } else if self.head.is_some() {
            self.display = Display::Diff;
        } else {
            self.message = Some("no diff base: not in a git repository".to_owned());
            return;
        }
        self.relayout();
    }

    /// `gD` / `:diff seen`: the unified diff against the last-seen
    /// snapshot (ADR 0015), or back to the rendered view.
    pub fn toggle_seen_diff_view(&mut self) {
        if self.display == Display::DiffSeen {
            self.display = Display::Rendered;
        } else if self.seen.is_some() {
            self.display = Display::DiffSeen;
        } else {
            self.message = Some("no last-seen snapshot of this file yet".to_owned());
            return;
        }
        self.relayout();
    }

    /// `]g` within the file: the cursor to the next hunk against `HEAD`.
    /// Reports a wrap instead of taking it, so the app can cross into the
    /// next dirty file (ADR 0017).
    pub fn next_hunk(&mut self) -> HunkStep {
        self.step_hunk(true)
    }

    /// `[g` within the file: the cursor to the previous hunk.
    pub fn prev_hunk(&mut self) -> HunkStep {
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
        // Anchor by source line and column, not byte offset, so an insertion
        // above the cursor does not drag it onto unrelated text (ADR 0010).
        let (line, column) = self.source_position();
        let screen_row = self.cursor.row.saturating_sub(self.scroll);
        let base = match self.display {
            Display::Diff => self.head.as_deref(),
            Display::DiffSeen => self.seen.as_deref(),
            _ => None,
        };
        self.layout = match (self.display, base) {
            (Display::Source, _) => Layout::source_with(
                &self.text,
                self.width,
                &self.syntax.hint,
                &self.syntax.highlighter,
            ),
            (Display::Diff | Display::DiffSeen, Some(base)) => {
                Layout::diff(base, &self.text, self.width)
            }
            _ => Layout::render_with(&self.text, self.width, &self.syntax.highlighter),
        };
        let index = self.layout.index();
        let line = line.min(index.line_count());
        let offset = index.offset_at(&self.text, line, column);
        let row = offset
            .and_then(|offset| self.layout.line_at_offset(offset))
            .unwrap_or(self.cursor.row);
        self.cursor.row = row.min(self.last_row());
        self.scroll = self.cursor.row.saturating_sub(screen_row);
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
        self.cursor.row = row.min(self.last_row());
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

    pub fn move_down(&mut self, n: usize) {
        self.set_row(self.cursor.row.saturating_add(n));
    }

    pub fn move_up(&mut self, n: usize) {
        self.set_row(self.cursor.row.saturating_sub(n));
    }

    pub fn half_page_down(&mut self) {
        self.move_down((self.height / 2).max(1));
    }

    pub fn half_page_up(&mut self) {
        self.move_up((self.height / 2).max(1));
    }

    pub fn goto_top(&mut self) {
        self.jump_to_row(0);
    }

    pub fn goto_bottom(&mut self) {
        self.jump_to_row(self.last_row());
    }

    /// Whether `h` has nowhere left to go on this row.
    #[must_use]
    pub fn at_line_start(&self) -> bool {
        !self
            .columns(self.cursor.row)
            .iter()
            .any(|&c| c < self.cursor.col)
    }

    /// One cell left; a selection wraps onto the end of the row above.
    pub fn move_left(&mut self) {
        let columns = self.columns(self.cursor.row);
        if let Some(&col) = columns.iter().rev().find(|&&c| c < self.cursor.col) {
            self.cursor.col = col;
        } else if self.mode == Mode::Select && self.cursor.row > 0 {
            self.cursor.row -= 1;
            self.cursor.col = self.columns(self.cursor.row).last().copied().unwrap_or(0);
            self.ensure_visible();
        }
        self.want_col = self.cursor.col;
        self.extend_selection();
    }

    pub fn move_right(&mut self) {
        let columns = self.columns(self.cursor.row);
        if let Some(&col) = columns.iter().find(|&&c| c > self.cursor.col) {
            self.cursor.col = col;
        }
        self.want_col = self.cursor.col;
        self.extend_selection();
    }

    pub fn line_start(&mut self) {
        self.cursor.col = 0;
        self.want_col = 0;
        self.extend_selection();
    }

    pub fn line_end(&mut self) {
        self.cursor.col = self.columns(self.cursor.row).last().copied().unwrap_or(0);
        self.want_col = usize::MAX;
        self.extend_selection();
    }

    /// Scroll the viewport without a cursor jump unless the cursor leaves it.
    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.max_scroll());
        let top = self.scroll;
        let bottom = self.scroll + self.height - 1;
        if self.cursor.row < top {
            self.cursor.row = top;
            self.clamp_col();
        } else if self.cursor.row > bottom {
            self.cursor.row = bottom.min(self.last_row());
            self.clamp_col();
        }
    }

    /// Place the cursor at a screen position (mouse click).
    pub fn click(&mut self, screen_row: usize, col: usize) {
        self.selection = None;
        self.mode = Mode::Normal;
        self.cursor.row = (self.scroll + screen_row).min(self.last_row());
        self.want_col = self.line_col(self.cursor.row, col);
        self.clamp_col();
    }

    /// Extend a mouse selection to a screen position (drag).
    pub fn drag(&mut self, screen_row: usize, col: usize) {
        let anchor = self.cursor;
        let row = (self.scroll + screen_row).min(self.last_row());
        let col = self.line_col(row, col);
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
    pub fn release(&mut self) {
        if self.selection.is_some() {
            self.mode = Mode::Select;
        }
    }

    /// Drop the selection and return to normal mode.
    pub fn clear_selection(&mut self) {
        self.selection = None;
        if self.mode == Mode::Select {
            self.mode = Mode::Normal;
        }
    }

    /// The 1-based source line rendered row `row` came from, if any.
    pub fn source_line_of_row(&self, row: usize) -> Option<usize> {
        let line = self.layout.lines().get(row)?;
        let range = line.source()?;
        Some(self.layout.index().line_of(range.start))
    }

    /// The source line under the cursor, or the nearest one above it.
    pub fn cursor_source_line(&self) -> Option<usize> {
        (0..=self.cursor.row)
            .rev()
            .find_map(|row| self.source_line_of_row(row))
    }

    /// Every source line rendered row `row` came from (a wrapped paragraph
    /// is one row for several lines).
    pub fn source_lines_of_row(&self, row: usize) -> Option<LineRange> {
        let line = self.layout.lines().get(row)?;
        let range = line.source()?;
        let index = self.layout.index();
        let last = index.line_of(range.end.max(range.start + 1) - 1);
        Some(LineRange::new(index.line_of(range.start), last))
    }

    /// The source lines the selection covers, whichever way it was made.
    pub fn selected_lines(&self) -> Option<LineRange> {
        let (start, end) = self.selection?.ordered();
        let first = (start.row..=end.row).find_map(|row| self.source_lines_of_row(row))?;
        let last = (start.row..=end.row)
            .rev()
            .find_map(|row| self.source_lines_of_row(row))?;
        Some(LineRange::new(first.start(), last.end()))
    }

    /// `v`: toggle a character selection anchored at the cursor.
    pub fn select_chars(&mut self) {
        self.toggle_select(false);
    }

    /// `V`: toggle a line selection anchored at the cursor.
    pub fn select_lines(&mut self) {
        self.toggle_select(true);
    }

    /// `x` (Helix semantics): select the whole cursor line; each further
    /// press takes in one more line below. A `v` selection widens to whole
    /// lines first, keeping its anchor.
    pub fn extend_line_below(&mut self) {
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
        self.selection = Some(Selection {
            anchor: self.cursor,
            head: self.cursor,
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
        self.selection = Some(Selection {
            anchor: self.cursor,
            head: self.cursor,
            linewise,
        });
        self.mode = Mode::Select;
    }

    /// Copy the selection (`y`) and leave select mode.
    pub fn yank(&mut self) -> Effect {
        let effect = self.copy_selection();
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
    pub fn selected_source(&self) -> Option<String> {
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
        let from = floor_char(&self.text, from);
        let to = ceil_char(&self.text, to);
        self.text.get(from..to).map(str::to_owned)
    }

    /// Esc: clear input, pending keys, selection, then search highlights.
    pub fn escape(&mut self) {
        self.message = None;
        self.count = 0;
        if matches!(self.mode, Mode::Command | Mode::Search { .. }) {
            self.mode = Mode::Normal;
            self.input.clear();
        } else if self.pending.is_some() {
            self.pending = None;
        } else if self.selection.is_some() {
            self.selection = None;
            self.mode = Mode::Normal;
        } else {
            self.clear_highlight();
        }
    }

    pub fn clear_highlight(&mut self) {
        self.pattern = None;
        self.matches.clear();
    }

    pub fn set_pending(&mut self, key: Option<char>) {
        self.pending = key;
    }

    pub fn clear_message(&mut self) {
        self.message = None;
    }

    pub fn start_command(&mut self) {
        self.mode = Mode::Command;
        self.input.clear();
        self.message = None;
    }

    pub fn start_search(&mut self, backward: bool) {
        self.mode = Mode::Search { backward };
        self.backward = backward;
        self.input.clear();
        self.message = None;
    }

    /// Type into the `:` or `/` line; searches update incrementally.
    pub fn input_char(&mut self, ch: char) {
        self.input.push(ch);
        self.incremental();
    }

    pub fn input_backspace(&mut self) {
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
    pub fn confirm(&mut self) -> Effect {
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
            "diff" => {
                self.toggle_diff_view();
                Effect::None
            }
            "diff seen" => {
                self.toggle_seen_diff_view();
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
    pub fn goto_source_line(&mut self, line: usize) {
        if let Some(row) = self.row_of_source_line(line) {
            self.jump_to_row(row);
        }
    }

    /// `n` / `N`: next match in the search direction, flipped by `reverse`.
    pub fn search_next(&mut self, reverse: bool) {
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
            self.reveal_column();
        }
    }

    /// Scroll sideways the least amount that shows the cursor's cell when
    /// it sits on an unwrapped line (ADR 0029); a wrapped line never moves.
    fn reveal_column(&mut self) {
        let Some(line) = self.layout.lines().get(self.cursor.row) else {
            return;
        };
        if !line.is_unwrapped() || self.cursor.col < line.fixed_cells() {
            return;
        }
        let avail = self
            .layout
            .width()
            .saturating_sub(line.fixed_cells())
            .max(1);
        self.hscroll
            .reveal(self.cursor.col - line.fixed_cells(), avail, &self.layout);
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

    use super::{Cursor, Effect, HunkStep, Mode, View};

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
    fn left_at_column_zero_stops_unless_selecting() {
        let mut v = view();
        v.move_down(2);
        assert!(v.at_line_start());
        v.move_left();
        assert_eq!(v.cursor().row, 2, "normal mode stays put at column 0");
        v.move_right();
        assert!(!v.at_line_start());
        v.select_chars();
        v.move_left();
        v.move_left();
        assert_eq!(v.cursor().row, 1, "a selection wraps onto the row above");
        assert_eq!(v.cursor().col, v.columns(1).last().copied().unwrap_or(0));
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
        v.set_pending(Some('g'));
        v.escape();
        assert_eq!(v.pending(), None);
        assert!(v.selection().is_some());
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
        v.toggle_diff_view();
        assert!(!v.diff_view(), "no base, no diff view");
        assert_eq!(v.message(), Some("no diff base: not in a git repository"));

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

        v.toggle_diff_view();
        assert!(v.diff_view());
        assert_eq!(v.source_position().0, 9, "toggle keeps the source line");
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
        v.confirm();
        assert!(!v.diff_view());

        // A reload against the same base re-diffs; an identical text is clean.
        v.reload("# Title\n\nalpha beta\n\n- one\n- three\n\nlast word here\n".to_owned());
        assert_eq!(v.diff_counts(), Some((0, 0)));
        assert_eq!(v.next_hunk(), HunkStep::Clean);
        v.toggle_diff_view();
        assert_eq!(v.layout().lines().len(), 1);
        v.set_bases(None, None, None);
        assert_eq!(v.diff_counts(), None);
        assert!(v.diff_view(), "display sticks; layout falls back");
        assert!(v.layout().lines().len() > 1);
    }

    #[test]
    fn head_and_seen_diffs_toggle_independently() {
        let mut v = view();
        v.toggle_seen_diff_view();
        assert!(!v.diff_view(), "never seen: no seen diff");
        assert!(v.message().is_some_and(|m| m.contains("last-seen")));

        let seen = v.text().replace("- two\n", "");
        let head = "# Title\n".to_owned();
        v.set_bases(Some(seen), Some(head.clone()), Some(head));
        assert!(
            v.diff_counts().is_some_and(|(added, _)| added > 1),
            "the gutter counts against HEAD, not seen"
        );
        assert_eq!(v.first_hunk_line(), Some(2));

        v.toggle_diff_view();
        assert!(v.diff_view());
        assert!(!v.diff_seen());
        v.toggle_diff_view();
        assert!(!v.diff_view(), "gd is a toggle");

        v.toggle_seen_diff_view();
        assert!(v.diff_view());
        assert!(v.diff_seen());
        let texts: Vec<String> = v
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .collect();
        assert!(texts.iter().any(|t| t == "+- two"), "{texts:?}");
        v.toggle_diff_view();
        assert!(
            v.diff_view() && !v.diff_seen(),
            "gd from the seen diff goes to HEAD"
        );
        v.start_command();
        for ch in "diff seen".chars() {
            v.input_char(ch);
        }
        assert_eq!(v.confirm(), Effect::None);
        assert!(v.diff_seen(), ":diff seen from the HEAD diff goes to seen");
        v.toggle_seen_diff_view();
        assert!(!v.diff_view());

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

    // ADR 0029: horizontal scroll.
    const WIDE: &str =
        "intro\n\n```\nabcdefghij_klmnopqrst_uvwxyz_needle_end\n```\n\nneedle in prose\n";

    #[test]
    fn search_reveals_a_match_on_an_unwrapped_line_but_goto_leaves_it() {
        let mut v = View::new(WIDE.to_owned(), 20, 10);
        v.start_search(false);
        for ch in "needle".chars() {
            v.input_char(ch);
        }
        v.confirm();
        assert_eq!(v.cursor().row, 2, "the code line holds the first match");
        // Column 29 shows with four cells to spare at the right edge.
        assert_eq!(v.column_offset(), 14);
        assert_eq!(v.screen_col(2, 29), Some(15));
        assert_eq!(v.screen_col(2, 3), None, "scrolled out of view");
        v.search_next(false);
        assert_eq!(v.cursor().row, 4, "the prose match");
        assert_eq!(v.column_offset(), 14, "a wrapped line never moves it");
        v.goto_source_line(4);
        assert_eq!(v.column_offset(), 14, ":N leaves the offset alone");
        assert_eq!(v.screen_col(4, 7), Some(7), "prose columns are unshifted");
    }

    #[test]
    fn column_offset_survives_reload_and_toggles_and_keeps_full_copies() {
        let mut v = View::new(WIDE.to_owned(), 20, 10);
        v.scroll_columns(10);
        assert_eq!(v.column_offset(), 10);
        v.reload(WIDE.replace("intro", "changed intro"));
        assert_eq!(v.column_offset(), 10, "a reload keeps the offset");
        v.toggle_source_view();
        assert_eq!(v.column_offset(), 10, "the source view keeps the offset");
        v.toggle_source_view();
        assert_eq!(v.column_offset(), 10);
        v.set_bases(None, None, Some("intro\n".to_owned()));
        v.toggle_diff_view();
        assert!(v.diff_view());
        assert_eq!(v.column_offset(), 10, "the diff view keeps the offset");
        v.scroll_columns(-100);
        assert_eq!(v.column_offset(), 0, "zh at the left edge stops");

        // A click on a shifted row lands on the source column it shows,
        // and a copy reads the whole source line.
        v.toggle_diff_view();
        v.scroll_columns(5);
        v.click(2, 0);
        assert_eq!(v.cursor().col, 5);
        v.select_lines();
        assert_eq!(
            v.selected_source().as_deref(),
            Some("abcdefghij_klmnopqrst_uvwxyz_needle_end")
        );
    }

    #[test]
    fn a_document_that_fits_ignores_horizontal_scroll() {
        let mut v = view();
        v.scroll_columns(5);
        assert_eq!(v.column_offset(), 0);
        assert_eq!(v.screen_col(0, 3), Some(3));
    }
}
