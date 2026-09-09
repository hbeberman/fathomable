// @okf-doc: /decisions/0025-thread-list.md
//! The review list (ADR 0025, reshaped by ADR 0049 and ADR 0066): every
//! thread on the current work by file and line under a row per file,
//! resolved ones hidden until asked for, drawn in place of the document.
//!
//! The list keeps only its own state — whether it is open, the scroll,
//! and which files are folded; the resolved flag and the file filter
//! are the [`ReviewState`] the sidebar's threads pane shares. Its rows
//! are computed from the store on every draw and key by
//! [`App::review_rows`], so a reload or a change of state needs
//! nothing invalidated.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use fathomable_core::annotations::{LineRange, Thread, ThreadId};
use fathomable_core::layout::{Layout, Line};

use crate::app::threads::ThreadState;
use crate::app::threads::words::Words;
use crate::app::{App, Focus};

/// Rows kept visible above and below the selected message.
const SCROLLOFF: usize = 2;
/// Cells a message body is indented from the column edge: two deeper than
/// its author row. The UI draws the indent before each body row; the
/// wrap width already accounts for it.
pub(crate) const BODY_INDENT: usize = 5;

/// What the review shows (ADR 0049), shared by the review list and the
/// sidebar's threads pane.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReviewState {
    /// Resolved threads are listed too; hidden by default.
    pub(crate) resolved: bool,
    /// Only the current document's threads (the list's `f`).
    pub(crate) file_only: bool,
}

/// The list's state; the rows are derived from the store.
#[derive(Debug, Default)]
pub(crate) struct ReviewList {
    open: bool,
    scroll: usize,
    /// Files folded to their row (ADR 0066); the list's own, apart from
    /// the threads pane's.
    folded: HashSet<PathBuf>,
}

impl ReviewList {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn scroll(&self) -> usize {
        self.scroll
    }

    /// Whether `path` is folded to its row.
    pub(crate) fn is_folded(&self, path: &Path) -> bool {
        self.folded.contains(path)
    }

    /// A document took the column back.
    pub(crate) fn close(&mut self) {
        self.open = false;
    }
}

/// The threads of one scope by state, for the headers' counts (ADR
/// 0066): `resolved` counts the resolved threads whether or not they
/// are listed, so the count says what `x` would reveal.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) open: usize,
    pub(crate) waiting: usize,
    pub(crate) resolved: usize,
}

/// The first line of an entry, for sorting; a thread on the file as a
/// whole has none and comes first.
fn start_of(entry: &Entry) -> Option<usize> {
    entry.range.map(|range| range.start())
}

/// The files pane's order for two root-relative paths (ADR 0066):
/// directories before files at each level, names case-insensitively
/// and then exactly, so the list groups files as the tree shows them
/// whether or not the tree is drawn.
pub(crate) fn tree_order(a: &Path, b: &Path) -> Ordering {
    let a: Vec<&std::ffi::OsStr> = a.iter().collect();
    let b: Vec<&std::ffi::OsStr> = b.iter().collect();
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        let x_dir = i + 1 < a.len();
        let y_dir = i + 1 < b.len();
        let order = match (x_dir, y_dir) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => {
                let x = x.to_string_lossy();
                let y = y.to_string_lossy();
                x.to_lowercase()
                    .cmp(&y.to_lowercase())
                    .then_with(|| x.cmp(&y))
            }
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.len().cmp(&b.len())
}

/// One thread in list order, for moving and acting on the selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    id: ThreadId,
    path: PathBuf,
    /// `None` for a thread on the file as a whole (ADR 0063).
    range: Option<LineRange>,
    words: Words,
    /// When the thread last changed.
    updated: u64,
    /// The branch of the worktree that shows the thread when the active
    /// one does not (ADR 0070).
    worktree: Option<String>,
}

impl Entry {
    /// The branch on the entry when another worktree shows the thread
    /// (ADR 0070).
    pub(crate) fn worktree(&self) -> Option<&str> {
        self.worktree.as_deref()
    }

    pub(crate) fn id(&self) -> &ThreadId {
        &self.id
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn range(&self) -> Option<LineRange> {
        self.range
    }

    pub(crate) fn kind(&self) -> ThreadState {
        self.words.state()
    }

    /// An agent's newest reply proposes resolving it (ADR 0053).
    #[cfg(test)]
    pub(crate) fn proposed(&self) -> bool {
        self.words.proposed()
    }

    /// The thread's words and circle (ADR 0032, ADR 0066).
    pub(crate) fn words(&self) -> Words {
        self.words
    }
}

/// One drawn row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Row {
    /// A file's row over its threads (ADR 0066): the path and how many
    /// threads it lists; `selected` when the cursor's thread is inside
    /// it while it is folded.
    File {
        path: PathBuf,
        count: usize,
        folded: bool,
        current: bool,
        selected: bool,
    },
    /// The first row of an entry: the circle, the range, the words,
    /// the age; `selected` for the cursor's thread.
    Header {
        entry: usize,
        /// `None` for a thread on the file as a whole (ADR 0063).
        range: Option<LineRange>,
        words: Words,
        updated: u64,
        selected: bool,
        dim: bool,
        /// The branch of the worktree showing it (ADR 0070).
        worktree: Option<String>,
    },
    /// `author  age  [badge]`.
    Message {
        entry: usize,
        message: usize,
        author: String,
        created: u64,
        badge: Option<&'static str>,
        dim: bool,
        selected: bool,
    },
    /// One row of a comment or reply rendered as Markdown (ADR 0037),
    /// wrapped to the column less [`BODY_INDENT`].
    Body {
        entry: usize,
        message: usize,
        line: Line,
        dim: bool,
        selected: bool,
    },
    Blank,
}

/// The computed list: rows to draw and entries to act on.
#[derive(Debug, Default)]
pub(crate) struct Rows {
    pub(crate) rows: Vec<Row>,
    pub(crate) entries: Vec<Entry>,
}

impl Rows {
    /// The row index of `entry`'s header.
    pub(crate) fn header_row(&self, entry: usize) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Header { entry: e, .. } if *e == entry))
    }

    /// The row index of the file row over `entry`.
    pub(crate) fn file_row_of(&self, entry: usize) -> Option<usize> {
        let path = self.entries.get(entry)?.path.as_path();
        self.rows
            .iter()
            .position(|row| matches!(row, Row::File { path: p, .. } if p == path))
    }

    /// The path of the file row at `row`, if it is one.
    pub(crate) fn file_at(&self, row: usize) -> Option<&Path> {
        match self.rows.get(row)? {
            Row::File { path, .. } => Some(path),
            _ => None,
        }
    }

    /// The first entry of `path`.
    pub(crate) fn first_entry_of(&self, path: &Path) -> Option<usize> {
        self.entries.iter().position(|entry| entry.path == path)
    }

    /// The entry and optional message under `row`.
    pub(crate) fn selection_at(&self, row: usize) -> Option<(usize, Option<usize>)> {
        match self.rows.get(row)? {
            Row::Header { entry, .. } => Some((*entry, None)),
            Row::Message { entry, message, .. } | Row::Body { entry, message, .. } => {
                Some((*entry, Some(*message)))
            }
            Row::File { .. } | Row::Blank => None,
        }
    }

    /// The row range occupied by one message.
    fn message_range(&self, entry: usize, message: usize) -> Option<std::ops::Range<usize>> {
        let start = self.rows.iter().position(
            |row| matches!(row, Row::Message { entry: e, message: m, .. } if *e == entry && *m == message),
        )?;
        let len = self.rows[start..]
            .iter()
            .take_while(|row| {
                matches!(
                    row,
                    Row::Message { entry: e, message: m, .. }
                        | Row::Body { entry: e, message: m, .. }
                        if *e == entry && *m == message
                )
            })
            .count();
        Some(start..start + len)
    }

    /// The entries that are stops for `j` / `k` (ADR 0066): every entry
    /// with a header, which is every entry of an unfolded file, and the
    /// first entry of a folded one.
    fn stops(&self) -> Vec<usize> {
        (0..self.entries.len())
            .filter(|&entry| self.is_stop(entry))
            .collect()
    }

    fn is_stop(&self, entry: usize) -> bool {
        self.header_row(entry).is_some()
            || self
                .entries
                .get(entry)
                .and_then(|e| self.first_entry_of(&e.path))
                == Some(entry)
    }

    /// The stop `entry` counts as: itself, or the first entry of its
    /// folded file.
    fn stop_of(&self, entry: usize) -> usize {
        if self.is_stop(entry) {
            return entry;
        }
        self.entries
            .get(entry)
            .and_then(|e| self.first_entry_of(&e.path))
            .unwrap_or(entry)
    }
}

fn is_open(kind: ThreadState) -> bool {
    matches!(kind, ThreadState::Open | ThreadState::Waiting)
}

impl App {
    /// The list, open or not.
    pub(crate) fn review_list(&self) -> &ReviewList {
        &self.review_list
    }

    /// `Space r`: show the list in place of the document, or focus it
    /// when it is open, or close it when it is open and focused.
    pub(crate) fn toggle_review(&mut self) {
        if self.review_list.is_open() && self.focus == Focus::Review {
            self.close_review();
        } else if self.review_list.is_open() {
            self.focus = Focus::Review;
        } else {
            self.open_review();
        }
    }

    /// Show the list in place of the document. The pane closes; the
    /// filter and folds are whatever they were last time, and the cursor
    /// is where the reader was (ADR 0046).
    pub(crate) fn open_review(&mut self) {
        if self.store.is_none() {
            self.store_mut();
            return;
        }
        // The cursor the text was on becomes the list's.
        let cursor = self.thread_cursor();
        if let Some(id) = cursor.thread().cloned() {
            self.set_thread_cursor_message(id, cursor.message());
        }
        self.review_list.open = true;
        self.focus = Focus::Review;
        let rows = self.review_rows(self.column_width());
        if rows.entries.is_empty() {
            self.notice(self.empty_review_notice());
        } else if let Some(index) = self.selected_index(&rows) {
            self.select_entry(&rows, index);
        }
        self.relayout();
    }

    fn empty_review_notice(&self) -> &'static str {
        if self.review.file_only {
            "no threads in this file"
        } else {
            "no threads in the workspace"
        }
    }

    /// What the review shows.
    pub(crate) fn review(&self) -> ReviewState {
        self.review
    }

    /// The threads the review lists, in its order (ADR 0049, ADR 0066):
    /// resolved ones only when asked for, narrowed to the current file
    /// when `file_only`; files in the files pane's order, threads by
    /// line.
    pub(crate) fn review_entries(&self, file_only: bool) -> Vec<Entry> {
        self.review_entries_showing(file_only, self.review.resolved)
    }

    /// [`App::review_entries`] with the resolved flag given, so a count
    /// can say what is hidden.
    fn review_entries_showing(&self, file_only: bool, resolved: bool) -> Vec<Entry> {
        let Some(store) = self.store.as_ref() else {
            return Vec::new();
        };
        let current = self.current_path();
        let mut entries: Vec<Entry> = store
            .threads()
            .iter()
            .filter(|thread| self.reach.includes(thread))
            .filter(|thread| !file_only || thread.path() == current)
            .filter_map(|thread| {
                let (range, words) = self.placement_of(thread);
                if !resolved && !is_open(words.state()) {
                    return None;
                }
                Some(Entry {
                    id: thread.id().clone(),
                    path: thread.path().to_path_buf(),
                    range,
                    words,
                    updated: thread.updated(),
                    worktree: self.worktree_of(thread.id()),
                })
            })
            .collect();
        entries.sort_by(|a, b| tree_order(&a.path, &b.path).then(start_of(a).cmp(&start_of(b))));
        entries
    }

    /// The threads of the review's scope by state (ADR 0066), the
    /// resolved ones counted whether or not they are listed.
    pub(crate) fn review_counts(&self, file_only: bool) -> Counts {
        let mut counts = Counts::default();
        for entry in self.review_entries_showing(file_only, true) {
            match entry.kind() {
                ThreadState::Open => counts.open += 1,
                ThreadState::Waiting => counts.waiting += 1,
                ThreadState::Resolved => counts.resolved += 1,
            }
        }
        counts
    }

    /// The circle each file with listed threads shows in the files pane
    /// (ADR 0066): its most urgent thread's, by path.
    pub(crate) fn file_circles(&self) -> Vec<(PathBuf, Words)> {
        let mut out: Vec<(PathBuf, Words)> = Vec::new();
        // The circles count what the active worktree reaches (ADR 0070).
        for entry in self
            .review_entries(false)
            .into_iter()
            .filter(|entry| entry.worktree.is_none())
        {
            match out.iter_mut().find(|(path, _)| path == entry.path()) {
                Some((_, words)) => {
                    if entry.words().urgency() > words.urgency() {
                        *words = entry.words();
                    }
                }
                None => out.push((entry.path().to_path_buf(), entry.words())),
            }
        }
        out
    }

    /// Esc: back to the document that was showing.
    pub(crate) fn close_review(&mut self) {
        if !self.review_list.open {
            return;
        }
        self.review_list.open = false;
        if self.focus == Focus::Review {
            self.focus = Focus::View;
        }
        self.relayout();
    }

    /// Columns the list has: the text column without the tree.
    pub(crate) fn column_width(&self) -> usize {
        self.width.saturating_sub(self.sidebar_width()).max(1)
    }

    /// The rows and entries for a list `width` cells wide, in review
    /// order: a row per file (unless the list is one file's), then each
    /// of its entries with its header and its messages, a blank row
    /// between entries; a folded file is its row alone.
    pub(crate) fn review_rows(&self, width: usize) -> Rows {
        let mut out = Rows::default();
        let body_width = width.saturating_sub(BODY_INDENT).max(1);
        let cursor = self.thread_cursor();
        let entries = self.review_entries(self.review.file_only);
        let grouped = !self.review.file_only;
        let mut index = 0;
        while index < entries.len() {
            let path = entries[index].path.clone();
            let group_end = entries[index..]
                .iter()
                .position(|entry| entry.path != path)
                .map_or(entries.len(), |len| index + len);
            let folded = grouped && self.review_list.folded.contains(&path);
            if grouped {
                let selected = folded
                    && entries[index..group_end]
                        .iter()
                        .any(|entry| cursor.thread() == Some(&entry.id));
                out.rows.push(Row::File {
                    path: path.clone(),
                    count: group_end - index,
                    folded,
                    current: path == self.current_path(),
                    selected,
                });
            }
            for entry in &entries[index..group_end] {
                let entry_index = out.entries.len();
                if !folded {
                    let selected = cursor.thread() == Some(&entry.id);
                    self.push_entry(
                        &mut out,
                        entry,
                        entry_index,
                        selected,
                        !is_open(entry.kind()),
                        body_width,
                    );
                }
                out.entries.push(entry.clone());
            }
            index = group_end;
        }
        out
    }

    /// Range and words of `thread`: from the loaded document's mark when
    /// its file is open this session, else as stored; no range for a
    /// thread on the file as a whole (ADR 0063).
    pub(super) fn placement_of(&self, thread: &Thread) -> (Option<LineRange>, Words) {
        // A thread another worktree shows is placed in that worktree's
        // file (ADR 0070).
        if let Some(placed) = self.elsewhere_words(thread.id()) {
            return placed;
        }
        self.docs
            .iter()
            .find(|doc| doc.relative == thread.path())
            .and_then(|doc| doc.marks.iter().find(|mark| mark.id() == thread.id()))
            .map_or_else(
                || (thread.range(), Words::of(None, thread)),
                |mark| (mark.range(), mark.words()),
            )
    }

    fn push_entry(
        &self,
        out: &mut Rows,
        entry: &Entry,
        index: usize,
        selected: bool,
        dim: bool,
        body_width: usize,
    ) {
        let Some(thread) = self.thread(&entry.id) else {
            return;
        };
        out.rows.push(Row::Header {
            entry: index,
            range: entry.range,
            words: entry.words,
            updated: thread.updated(),
            selected,
            dim,
            worktree: entry.worktree.clone(),
        });
        let selected_message =
            selected.then(|| self.thread_cursor().message().min(thread.replies().len()));
        let mut message = |message: usize,
                           author: &str,
                           created: u64,
                           body: &str,
                           badge: Option<&'static str>| {
            let message_selected = selected_message == Some(message);
            out.rows.push(Row::Message {
                entry: index,
                message,
                author: author.to_owned(),
                created,
                badge,
                dim,
                selected: message_selected,
            });
            // The body as the expanded thread in the file draws it (ADR
            // 0037): Markdown, a newline kept as a line break, fences
            // coloured by the app's highlighter.
            for line in Layout::render_message(body, body_width, self.highlighter()).lines() {
                out.rows.push(Row::Body {
                    entry: index,
                    message,
                    line: line.clone(),
                    dim,
                    selected: message_selected,
                });
            }
        };
        // The comment's author as a reply's is shown (ADR 0061).
        let comment_author = if thread.author().is_user() {
            self.user_name()
        } else {
            thread.author().name()
        };
        message(0, comment_author, thread.created(), thread.comment(), None);
        for (reply_index, reply) in thread.replies().iter().enumerate() {
            let badge = reply.proposes_resolution().then_some("proposes resolving");
            message(
                reply_index + 1,
                reply.author().name(),
                reply.created(),
                reply.body(),
                badge,
            );
        }
        out.rows.push(Row::Blank);
    }

    /// Rows the list has for its entries: the column minus its header
    /// and its key bar (ADR 0059).
    fn list_rows(&self) -> usize {
        self.text_rows().saturating_sub(2).max(1)
    }

    /// The cursor's entry, or the first entry when the cursor's thread
    /// is not listed.
    fn selected_index(&self, rows: &Rows) -> Option<usize> {
        let cursor = self.thread_cursor();
        let by_id = cursor
            .thread()
            .and_then(|id| rows.entries.iter().position(|entry| &entry.id == id));
        by_id.or_else(|| (!rows.entries.is_empty()).then_some(0))
    }

    /// Put the cursor on entry `index`, a new thread at its newest message.
    fn select_entry(&mut self, rows: &Rows, index: usize) {
        let Some(entry) = rows.entries.get(index) else {
            return;
        };
        self.set_thread_cursor(entry.id.clone());
        self.scroll_to_selection(rows, index);
    }

    fn select_message(&mut self, rows: &Rows, entry: usize, message: usize) {
        let Some(id) = rows.entries.get(entry).map(|entry| entry.id.clone()) else {
            return;
        };
        self.set_thread_cursor_message(id, message);
        self.scroll_to_selection(rows, entry);
    }

    /// Scroll enough to keep the cursor's message, or a folded file's
    /// row, visible; the entry is re-found when the rows changed under it.
    pub(crate) fn review_follow_cursor(&mut self) {
        let rows = self.review_rows(self.column_width());
        if let Some(index) = self.selected_index(&rows) {
            self.select_entry(&rows, index);
        }
    }

    /// Scroll enough to keep the selected message, or the folded file's
    /// row, visible.
    fn scroll_to_selection(&mut self, rows: &Rows, entry: usize) {
        let range = rows
            .message_range(entry, self.thread_cursor().message())
            .or_else(|| rows.header_row(entry).map(|row| row..row + 1))
            .or_else(|| rows.file_row_of(entry).map(|row| row..row + 1));
        let Some(range) = range else {
            return;
        };
        let visible = self.list_rows();
        let top = range.start.saturating_sub(SCROLLOFF);
        let bottom = (range.end + SCROLLOFF).min(rows.rows.len());
        if top < self.review_list.scroll {
            self.review_list.scroll = top;
        } else if bottom > self.review_list.scroll + visible {
            self.review_list.scroll = if range.len() > visible {
                range.start
            } else {
                bottom.saturating_sub(visible)
            };
        }
    }

    /// `j` / `k`: move between threads by `delta` in the list's order,
    /// a folded file being one stop (ADR 0066).
    pub(crate) fn review_step(&mut self, delta: isize) {
        let rows = self.review_rows(self.column_width());
        let Some(index) = self.selected_index(&rows) else {
            return;
        };
        let stops = rows.stops();
        let stop = rows.stop_of(index);
        let at = stops.iter().position(|&s| s == stop).unwrap_or(0);
        let last = stops.len().saturating_sub(1);
        let target = stops[at.saturating_add_signed(delta).min(last)];
        self.select_entry(&rows, target);
    }

    /// `Ctrl-d` / `Ctrl-u`: the message half a page of rows below or
    /// above the highlighted one, the nearest when that row is a heading.
    pub(crate) fn review_page(&mut self, direction: isize) {
        let rows = self.review_rows(self.column_width());
        let Some(entry) = self.selected_index(&rows) else {
            return;
        };
        let Some(start) = rows
            .message_range(entry, self.thread_cursor().message())
            .map(|range| range.start)
            .or_else(|| rows.header_row(entry))
            .or_else(|| rows.file_row_of(entry))
        else {
            return;
        };
        let half = (self.list_rows() / 2).max(1);
        let last = rows.rows.len().saturating_sub(1);
        let target = if direction > 0 {
            (start + half).min(last)
        } else {
            start.saturating_sub(half)
        };
        let landing = if direction > 0 {
            (target..=last).find_map(|row| rows.selection_at(row))
        } else {
            (0..=target).rev().find_map(|row| rows.selection_at(row))
        };
        match landing {
            Some((entry, Some(message))) => self.select_message(&rows, entry, message),
            Some((entry, None)) => self.select_entry(&rows, entry),
            None => {}
        }
    }

    /// `gg` / `G`.
    pub(crate) fn review_goto(&mut self, end: bool) {
        let rows = self.review_rows(self.column_width());
        let target = if end {
            rows.stops().last().copied().unwrap_or(0)
        } else {
            0
        };
        self.select_entry(&rows, target);
        if !end {
            self.review_list.scroll = 0;
        }
    }

    /// The wheel: scroll the rows; the selection stays where it is.
    pub(crate) fn review_scroll(&mut self, delta: isize) {
        let rows = self.review_rows(self.column_width());
        let max = rows.rows.len().saturating_sub(self.list_rows());
        self.review_list.scroll = self
            .review_list
            .scroll
            .saturating_add_signed(delta)
            .min(max);
    }

    /// A click on list row `row` (below the header) selects its message;
    /// on a file row it folds or unfolds the file (ADR 0066).
    pub(crate) fn review_click(&mut self, row: usize) {
        let rows = self.review_rows(self.column_width());
        let at = self.review_list.scroll + row;
        if let Some(path) = rows.file_at(at).map(Path::to_path_buf) {
            self.review_toggle_fold(&path);
        } else if let Some((entry, message)) = rows.selection_at(at) {
            if let Some(message) = message {
                self.select_message(&rows, entry, message);
            } else {
                self.select_entry(&rows, entry);
            }
        }
        self.focus = Focus::Review;
    }

    /// A right-click on list row `row` (ADR 0066): on a file row the
    /// cursor goes to the file's first thread and the path is returned
    /// for the file's menu; elsewhere the click selects as a left one.
    pub(crate) fn review_point(&mut self, row: usize) -> Option<PathBuf> {
        let rows = self.review_rows(self.column_width());
        let at = self.review_list.scroll + row;
        let path = rows.file_at(at).map(Path::to_path_buf)?;
        if let Some(entry) = rows.first_entry_of(&path) {
            self.select_entry(&rows, entry);
        }
        self.focus = Focus::Review;
        Some(path)
    }

    /// `f`: narrow to the current file, or widen again.
    pub(crate) fn review_toggle_file(&mut self) {
        self.review.file_only = !self.review.file_only;
        self.reshow_review();
    }

    /// `x` in the list or the threads pane: list resolved threads too,
    /// or hide them again (ADR 0049).
    pub(crate) fn review_toggle_resolved(&mut self) {
        self.review.resolved = !self.review.resolved;
        self.notice(if self.review.resolved {
            "resolved shown"
        } else {
            "resolved hidden"
        });
        self.reshow_review();
    }

    /// The rows changed under the list: keep the cursor's entry in view,
    /// or say why the list is empty.
    fn reshow_review(&mut self) {
        if !self.review_list.is_open() {
            return;
        }
        self.review_list.scroll = 0;
        let rows = self.review_rows(self.column_width());
        if rows.entries.is_empty() {
            self.notice(self.empty_review_notice());
        } else if let Some(index) = self.selected_index(&rows) {
            self.select_entry(&rows, index);
        }
    }

    /// `z`: fold the cursor's file to its row, or unfold it (ADR 0066).
    pub(crate) fn review_fold(&mut self) {
        let rows = self.review_rows(self.column_width());
        let Some(index) = self.selected_index(&rows) else {
            return;
        };
        let path = rows.entries[index].path.clone();
        self.review_toggle_fold(&path);
    }

    /// `Z`: fold every listed file, or unfold them all when any is
    /// folded (ADR 0066).
    pub(crate) fn review_fold_all(&mut self) {
        let rows = self.review_rows(self.column_width());
        let listed: HashSet<PathBuf> = rows
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        if listed
            .iter()
            .any(|path| self.review_list.folded.contains(path))
        {
            self.review_list.folded.clear();
        } else {
            self.review_list.folded = listed;
        }
        self.review_follow_cursor();
    }

    fn review_toggle_fold(&mut self, path: &Path) {
        if !self.review_list.folded.remove(path) {
            self.review_list.folded.insert(path.to_path_buf());
        }
        self.review_follow_cursor();
    }

    /// The selected entry's index, for a caller about to change the store.
    pub(crate) fn review_selected_index(&self) -> Option<usize> {
        if !self.review_list.is_open() {
            return None;
        }
        self.selected_index(&self.review_rows(self.column_width()))
    }

    /// Re-select after the store changed under the list: the entry with
    /// the selected id, else the one now at `place` (the index before
    /// the change), else the last, else nothing.
    pub(crate) fn review_reselect(&mut self, place: Option<usize>) {
        if !self.review_list.is_open() {
            return;
        }
        let rows = self.review_rows(self.column_width());
        let cursor = self.thread_cursor();
        let index = self
            .selected_index(&rows)
            .filter(|_| {
                cursor
                    .thread()
                    .is_some_and(|id| rows.entries.iter().any(|entry| &entry.id == id))
            })
            .or_else(|| place.map(|place| place.min(rows.entries.len().saturating_sub(1))));
        if let Some(index) = index {
            self.select_entry(&rows, index);
        }
    }
}
