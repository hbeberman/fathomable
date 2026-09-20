// @okf-doc: /decisions/0025-thread-list.md
//! The review list (ADR 0025, reshaped by ADR 0049 and ADR 0066): the
//! shared board, Recently resolved, and Archived threads use one list
//! engine and one set of summary rows.
//!
//! The list keeps only its own state — whether it is open, the scroll,
//! which files and threads are folded (ADR 0076), and whether the
//! cursor rests on a file row; the resolved flag and the file filter
//! are the [`ReviewState`] the sidebar's threads pane shares. Its rows
//! are computed from the store on every draw and key by
//! [`App::review_rows`], so a reload or a change of state needs
//! nothing invalidated. The stops `j`/`k` walk are rows: a file row,
//! then each of its threads while it is unfolded, a folded thread
//! being one row.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use fathomable_core::annotations::{
    Author, LineRange, OriginVersion, Placement, Status, Store, Thread, ThreadId,
};
use fathomable_core::layout::{Layout, Line};

use crate::app::draw::nest::NEST;
use crate::app::threads::summary::ThreadSummary;
use crate::app::threads::words::Words;
use crate::app::threads::{ThreadState, author_label};
use crate::app::{App, Focus};

/// Rows kept visible above and below the selected message.
const SCROLLOFF: usize = 2;
/// Cells a message body is indented from the column edge: the nest
/// (ADR 0077), then two deeper than its author row. The UI draws the
/// indent before each body row; the wrap width already accounts for it.
pub(crate) const BODY_INDENT: usize = NEST + 5;

/// Which repository-board view the review list shows.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewView {
    /// Unarchived board threads, with the normal lifecycle filter.
    #[default]
    Board,
    /// Unarchived threads whose current lifecycle is resolved.
    RecentlyResolved,
    /// Archived threads, regardless of lifecycle.
    Archived,
}

fn origin_evidence(thread: &Thread) -> String {
    let side = match thread.origin_side() {
        fathomable_core::annotations::OriginSide::Base => "source",
        fathomable_core::annotations::OriginSide::Target => "target",
        fathomable_core::annotations::OriginSide::Unspecified => "unspecified",
    };
    let version = origin_label(thread).unwrap_or_else(|| "origin unknown".to_owned());
    let location = thread
        .origin()
        .range()
        .map_or_else(|| "file".to_owned(), |range| format!("L{range}"));
    let excerpt = if thread.origin().snippet().is_empty() {
        "[file-wide]".to_owned()
    } else {
        thread.origin().snippet().to_owned()
    };
    let context = thread
        .origin()
        .context()
        .map(|context| format!("\ncontext: {}", context.text().trim_end()))
        .unwrap_or_default();
    format!(
        "origin: {version} · {side} · {}:{location}\nexcerpt: {excerpt}{context}",
        thread.origin().path().display()
    )
}

impl ReviewView {
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Board => "Threads",
            Self::RecentlyResolved => "Recently resolved",
            Self::Archived => "Archived threads",
        }
    }
}

/// What the review shows (ADR 0049), shared by the review list and the
/// sidebar's threads pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReviewState {
    /// The board view or one of its explicit history views.
    pub(crate) view: ReviewView,
    /// Resolved threads are listed too; hidden by default.
    pub(crate) resolved: bool,
    /// Only the current document's threads (the list's `f`).
    pub(crate) file_only: bool,
}

impl Default for ReviewState {
    fn default() -> Self {
        Self {
            view: ReviewView::Board,
            resolved: false,
            file_only: false,
        }
    }
}

/// The list's state; the rows are derived from the store.
#[derive(Debug, Default)]
pub(crate) struct ReviewList {
    open: bool,
    scroll: usize,
    /// Files folded to their row (ADR 0066); the list's own, apart from
    /// the threads pane's.
    pub(super) folded: HashSet<PathBuf>,
    /// Threads folded to one row (ADR 0076), kept for the session.
    pub(super) folded_threads: HashSet<ThreadId>,
    /// The cursor rests on its thread's file row (ADR 0076), the stop
    /// over the file's threads; the thread cursor is the file's first.
    pub(super) on_file: bool,
    /// A navigation fallback that must show immutable origin evidence.
    evidence: Option<ThreadId>,
}

impl ReviewList {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn scroll(&self) -> usize {
        self.scroll
    }
}

/// The threads of one scope by circle, for the headers' counts (ADR
/// 0066, ADR 0075): a proposed thread counts only under `proposed`, and
/// `resolved` counts the resolved threads whether
/// or not they are listed, so the count says what `x` would reveal.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) active: usize,
    pub(crate) proposed: usize,
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
    /// The commit a past thread was resolved at, short, when no
    /// checkout shows it (ADR 0072).
    commit: Option<String>,
    /// Original evidence shown when current placement is historical or
    /// unavailable.
    evidence: Option<String>,
    summary: ThreadSummary,
}

/// A commit as an entry names it: its first seven hex digits.
pub(crate) fn short_commit(commit: &str) -> String {
    commit.chars().take(7).collect()
}

fn origin_label(thread: &Thread) -> Option<String> {
    match thread.origin_version() {
        OriginVersion::Commit { id } => Some(format!("origin {}", short_commit(id))),
        OriginVersion::WorkingTree { .. } => Some("origin working tree".to_owned()),
        OriginVersion::Index { .. } => Some("origin index".to_owned()),
        OriginVersion::ReviewPoint { id, .. } => Some(format!("origin point {id}")),
        OriginVersion::EmptyTree => Some("origin empty tree".to_owned()),
        OriginVersion::Unknown => None,
    }
}

impl Entry {
    /// The branch on the entry when another worktree shows the thread
    /// (ADR 0070).
    /// The commit on the entry when the thread is resolved at an
    /// earlier commit of this branch (ADR 0072).
    #[cfg(test)]
    pub(crate) fn commit(&self) -> Option<&str> {
        self.commit.as_deref()
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
        self.words.state() == ThreadState::Proposed
    }

    /// The thread's words and circle (ADR 0032, ADR 0066).
    pub(crate) fn words(&self) -> Words {
        self.words
    }

    pub(crate) fn summary(&self) -> &ThreadSummary {
        &self.summary
    }
}

/// One drawn row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Row {
    /// A file's row over its threads (ADR 0066): the path and how many
    /// threads it lists; `inside` when the cursor's thread is one of
    /// the file's, for the bar (ADR 0077); `selected` when the cursor
    /// rests on the row itself, on it as a stop or over a folded file.
    File {
        path: PathBuf,
        count: usize,
        folded: bool,
        current: bool,
        inside: bool,
        selected: bool,
    },
    /// Immutable origin evidence shown for archived/history entries.
    Evidence {
        entry: usize,
        line: Line,
        dim: bool,
        selected: bool,
    },
    /// An expanded entry's shared one-row summary.
    Header {
        entry: usize,
        summary: ThreadSummary,
        selected: bool,
        dim: bool,
    },
    /// A folded entry's shared one-row summary.
    Stub {
        entry: usize,
        summary: ThreadSummary,
        selected: bool,
        dim: bool,
    },
    /// `author  age  [badge]`; `user` when the author is the user, for
    /// the stripe and the name's colour (ADR 0071).
    Message {
        entry: usize,
        message: usize,
        user: bool,
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
        user: bool,
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

/// One stop of the list's `j`/`k` (ADR 0076): a file row, or a thread
/// with a header or a folded row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stop {
    File(PathBuf),
    Entry(usize),
}

/// What a click on a row lands on: a stop, or a message of an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Landing {
    Stop(Stop),
    Message(usize, usize),
}

impl Rows {
    /// The row index of `entry`'s header, or of its folded row.
    pub(crate) fn entry_row(&self, entry: usize) -> Option<usize> {
        self.rows.iter().position(|row| {
            matches!(row, Row::Header { entry: e, .. } | Row::Stub { entry: e, .. } if *e == entry)
        })
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

    /// What `row` lands on: a file row, a thread's header or folded
    /// row, or a message; nothing on a blank row.
    pub(crate) fn landing_at(&self, row: usize) -> Option<Landing> {
        match self.rows.get(row)? {
            Row::File { path, .. } => Some(Landing::Stop(Stop::File(path.clone()))),
            Row::Header { entry, .. } | Row::Stub { entry, .. } => {
                Some(Landing::Stop(Stop::Entry(*entry)))
            }
            Row::Message { entry, message, .. } | Row::Body { entry, message, .. } => {
                Some(Landing::Message(*entry, *message))
            }
            Row::Evidence { .. } | Row::Blank => None,
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

    /// The stops of `j` / `k` in row order (ADR 0076): every file row,
    /// and every thread with a header or a folded row, which is every
    /// thread of an unfolded file.
    fn stops(&self) -> Vec<Stop> {
        self.rows
            .iter()
            .filter_map(|row| match row {
                Row::File { path, .. } => Some(Stop::File(path.clone())),
                Row::Header { entry, .. } | Row::Stub { entry, .. } => Some(Stop::Entry(*entry)),
                Row::Message { .. } | Row::Body { .. } | Row::Evidence { .. } | Row::Blank => None,
            })
            .collect()
    }

    /// The row of `stop`.
    fn row_of_stop(&self, stop: &Stop) -> Option<usize> {
        match stop {
            Stop::File(path) => self
                .rows
                .iter()
                .position(|row| matches!(row, Row::File { path: p, .. } if p == path)),
            Stop::Entry(entry) => self.entry_row(*entry),
        }
    }
}

fn is_open(kind: ThreadState) -> bool {
    matches!(kind, ThreadState::Active | ThreadState::Proposed)
}

impl App {
    /// The list, open or not.
    pub(crate) fn review_list(&self) -> &ReviewList {
        &self.review_list
    }

    /// Show Reviews in place of the document and give it focus.
    ///
    /// The pane closes; the filter and folds are whatever they were last
    /// time, and the cursor is where the reader was (ADR 0046).
    pub(crate) fn open_review(&mut self) {
        if self.review_list.is_open() {
            self.focus = Focus::Review;
            return;
        }
        self.open_review_view(ReviewView::Board);
    }

    /// Open an explicit board history view in place of the document.
    pub(crate) fn open_review_view(&mut self, view: ReviewView) {
        self.open_review_view_with_evidence(view, None);
    }

    fn open_review_view_with_evidence(&mut self, view: ReviewView, evidence: Option<ThreadId>) {
        self.getting_started = None;
        if self.store.is_none() {
            self.store_mut();
            return;
        }
        self.review_list.evidence = evidence;
        // The cursor the text was on becomes the list's.
        let cursor = self.thread_cursor();
        if let Some(id) = cursor.thread().cloned() {
            self.set_thread_cursor_message(id, cursor.message());
        }
        self.review.view = view;
        if view != ReviewView::Board {
            self.review.file_only = false;
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

    /// Show the exact thread in Reviews with its containing rows expanded.
    pub(crate) fn show_thread_in_review(&mut self, id: &ThreadId) -> bool {
        let Some((path, archived, resolved)) = self.thread(id).map(|thread| {
            (
                self.thread_path(thread).to_path_buf(),
                thread.is_archived(),
                thread.status() == Status::Resolved,
            )
        }) else {
            return false;
        };
        let view = if archived {
            ReviewView::Archived
        } else {
            ReviewView::Board
        };
        self.review.view = view;
        self.review.resolved = resolved;
        self.review.file_only = false;
        self.review_list.folded.remove(&path);
        self.review_list.folded_threads.remove(id);
        self.review_list.on_file = false;
        self.set_thread_cursor(id.clone());
        self.open_review_view_with_evidence(view, Some(id.clone()));
        self.review_follow_cursor();
        true
    }

    fn empty_review_notice(&self) -> &'static str {
        if self.review.file_only {
            "no threads in this file"
        } else {
            match self.review.view {
                ReviewView::Board => "no threads in the workspace",
                ReviewView::RecentlyResolved => "no recently resolved threads",
                ReviewView::Archived => "no archived threads",
            }
        }
    }

    /// What the review shows.
    pub(crate) fn review(&self) -> ReviewState {
        self.review
    }

    /// The threads the review lists, in its order (ADR 0049, ADR 0066):
    /// resolved ones only when asked for, the past ones among them with
    /// their commit (ADR 0072), narrowed to the current file when
    /// `file_only`; files in the files pane's order, threads by line.
    pub(crate) fn review_entries(&self, file_only: bool) -> Vec<Entry> {
        self.review_entries_for(self.review.view, file_only, self.review.resolved, true)
    }

    /// Normal-board entries for sidebar and file-list surfaces, independent
    /// of whichever explicit history view occupies the main pane.
    pub(crate) fn normal_review_entries(&self, file_only: bool) -> Vec<Entry> {
        self.review_entries_for(ReviewView::Board, file_only, self.review.resolved, false)
    }

    fn review_entries_for(
        &self,
        view: ReviewView,
        file_only: bool,
        resolved: bool,
        allow_evidence: bool,
    ) -> Vec<Entry> {
        let Some(store) = self.store.as_ref() else {
            return Vec::new();
        };
        let current = self.current_path();
        let candidates: Vec<&Thread> = match view {
            ReviewView::Board => store
                .threads()
                .iter()
                .filter(|thread| {
                    self.normal_thread(thread)
                        || (allow_evidence
                            && self.review_list.evidence.as_ref() == Some(thread.id()))
                })
                .collect(),
            ReviewView::RecentlyResolved => store.recently_resolved(),
            ReviewView::Archived => store.archived_threads().iter().collect(),
        };
        let mut entries: Vec<Entry> = candidates
            .into_iter()
            .filter(|thread| {
                let path = if view == ReviewView::Archived {
                    thread.origin().path()
                } else {
                    self.thread_path(thread)
                };
                !file_only || path == current
            })
            .filter_map(|thread| {
                let exact = self.thread_matches_presentation(thread) == Some(true);
                let (placement, words) = self.placement_of(thread, exact);
                let range = placement.range();
                if view == ReviewView::Board && !resolved && !is_open(words.state()) {
                    return None;
                }
                let worktree = (!exact).then(|| self.worktree_of(thread.id())).flatten();
                let commit = (view == ReviewView::Board
                    && worktree.is_none()
                    && !self.normal_thread(thread))
                .then(|| thread.commit().map(short_commit))
                .flatten();
                let origin_context = (view == ReviewView::Archived)
                    .then(|| origin_label(thread))
                    .flatten();
                let context = worktree
                    .as_deref()
                    .or(commit.as_deref())
                    .or(origin_context.as_deref());
                let summary =
                    ThreadSummary::new(thread, Some(placement), self.user_name(), context);
                let evidence = (view != ReviewView::Board
                    || worktree.is_some()
                    || commit.is_some()
                    || placement.is_detached()
                    || self.review_list.evidence.as_ref() == Some(thread.id()))
                .then(|| origin_evidence(thread));
                Some(Entry {
                    id: thread.id().clone(),
                    path: if view == ReviewView::Archived {
                        thread.origin().path().to_path_buf()
                    } else {
                        self.thread_path(thread).to_path_buf()
                    },
                    range,
                    words,
                    updated: thread.modified(),
                    worktree,
                    commit,
                    evidence,
                    summary,
                })
            })
            .collect();
        match view {
            ReviewView::RecentlyResolved => entries.sort_by(|a, b| {
                let a_time = self
                    .thread(&a.id)
                    .and_then(Thread::latest_resolution)
                    .map(|record| (record.created(), record.ordinal()))
                    .unwrap_or_default();
                let b_time = self
                    .thread(&b.id)
                    .and_then(Thread::latest_resolution)
                    .map(|record| (record.created(), record.ordinal()))
                    .unwrap_or_default();
                b_time.cmp(&a_time).then_with(|| a.id.cmp(&b.id))
            }),
            ReviewView::Board | ReviewView::Archived => entries
                .sort_by(|a, b| tree_order(&a.path, &b.path).then(start_of(a).cmp(&start_of(b)))),
        }
        entries
    }

    /// The threads of the review's scope by circle (ADR 0066, ADR
    /// 0075), the resolved ones counted whether or not they are listed.
    pub(crate) fn review_counts(&self, file_only: bool) -> Counts {
        let mut counts = Counts::default();
        let current = self.current_path();
        for thread in self
            .store
            .iter()
            .flat_map(Store::threads)
            .filter(|thread| self.normal_thread(thread))
            .filter(|thread| !file_only || self.thread_path(thread) == current)
        {
            match ThreadState::of(thread) {
                ThreadState::Active => counts.active += 1,
                ThreadState::Proposed => counts.proposed += 1,
                ThreadState::Resolved => counts.resolved += 1,
            }
        }
        counts
    }

    /// Active, proposed, and resolved threads under `directory`.
    pub(crate) fn directory_thread_counts(&self, directory: &Path) -> Counts {
        let mut counts = Counts::default();
        for thread in self
            .store
            .iter()
            .flat_map(Store::threads)
            .filter(|thread| {
                self.thread_matches_presentation(thread)
                    .unwrap_or_else(|| self.reach.here(thread))
            })
            .filter(|thread| self.thread_path(thread).starts_with(directory))
            .filter(|thread| {
                self.thread_matches_presentation(thread).unwrap_or(false)
                    || self.worktree_of(thread.id()).is_none()
            })
        {
            match ThreadState::of(thread) {
                ThreadState::Active => counts.active += 1,
                ThreadState::Proposed => counts.proposed += 1,
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
            .normal_review_entries(false)
            .into_iter()
            .filter(|entry| {
                entry.worktree.is_none()
                    && self.thread(entry.id()).is_some_and(|thread| {
                        self.thread_matches_presentation(thread)
                            .unwrap_or_else(|| self.reach.here(thread))
                    })
            })
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
        self.review_list.evidence = None;
        self.review.view = ReviewView::Board;
        if self.focus == Focus::Review {
            self.focus = Focus::View;
        }
        self.reconcile_normal_thread_cursor();
        self.relayout();
    }

    /// Show the current file surface and give it focus.
    pub(crate) fn open_file_view(&mut self) {
        self.close_review();
        self.focus = Focus::View;
        self.relayout();
        self.resume_draft();
    }

    /// Reveal the exact cursor thread without changing filters, folds, or focus.
    pub(super) fn reveal_review_cursor(&mut self) {
        if !self.review_list.open {
            return;
        }
        let rows = self.review_rows(self.column_width());
        let index = self
            .thread_cursor()
            .thread()
            .and_then(|id| rows.entries.iter().position(|entry| entry.id() == id));
        if let Some(index) = index {
            self.review_list.on_file = false;
            self.scroll_to_selection(&rows, index);
        }
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
        // The cursor rests on its file's row (ADR 0076) while the list
        // says so, or while the file is folded over its thread.
        let cursor_path = cursor
            .thread()
            .and_then(|id| entries.iter().find(|entry| &entry.id == id))
            .map(|entry| entry.path.clone());
        let on_file = grouped && self.review_list.on_file && cursor_path.is_some();
        let mut index = 0;
        while index < entries.len() {
            let path = entries[index].path.clone();
            let group_end = entries[index..]
                .iter()
                .position(|entry| entry.path != path)
                .map_or(entries.len(), |len| index + len);
            let folded = grouped && self.review_list.folded.contains(&path);
            let cursor_inside = cursor_path.as_deref() == Some(path.as_path());
            if grouped {
                out.rows.push(Row::File {
                    path: path.clone(),
                    count: group_end - index,
                    folded,
                    current: path == self.current_path(),
                    inside: cursor_inside,
                    selected: cursor_inside && (on_file || folded),
                });
            }
            for entry in &entries[index..group_end] {
                let entry_index = out.entries.len();
                if !folded {
                    let selected = !on_file && cursor.thread() == Some(&entry.id);
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
    pub(super) fn placement_of(
        &self,
        thread: &Thread,
        exact: bool,
    ) -> (fathomable_core::annotations::Placement, Words) {
        if thread.is_archived() {
            let placement = thread
                .origin()
                .range()
                .map_or(Placement::File, Placement::Detached);
            return (placement, Words::of(Some(placement), thread));
        }
        // Exact membership belongs to the installed document. Only a
        // reach-retained origin can borrow another worktree's placement.
        if !exact && let Some(placement) = self.elsewhere_placement(thread.id()) {
            return (placement, Words::of(Some(placement), thread));
        }
        let loaded = self
            .docs
            .iter()
            .find(|doc| doc.relative == self.thread_path(thread))
            .and_then(|doc| doc.marks.iter().find(|mark| mark.id() == thread.id()));
        if let Some(mark) = loaded {
            return (mark.placement(), mark.words());
        }
        let detached = || {
            thread.range().map_or(
                fathomable_core::annotations::Placement::File,
                fathomable_core::annotations::Placement::Detached,
            )
        };
        if exact {
            let placement = self
                .installed_display_text(self.thread_path(thread))
                .ok()
                .flatten()
                .map_or_else(detached, |text| {
                    let hashes = fathomable_core::annotations::LineHashes::of(&text);
                    App::project_placement(thread, &text, &hashes)
                });
            return (placement, Words::of(Some(placement), thread));
        }
        let absolute = self.workspace().root().join(self.thread_path(thread));
        let placement = if absolute.is_file() {
            std::fs::read_to_string(absolute).map_or_else(
                |_| detached(),
                |text| {
                    let hashes = fathomable_core::annotations::LineHashes::of(&text);
                    App::project_placement(thread, &text, &hashes)
                },
            )
        } else {
            detached()
        };
        (placement, Words::of(Some(placement), thread))
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
        let user = self.user_name();
        // A folded thread is one row, the stub's form, with no blank row
        // after it (ADR 0076).
        if self.review_list.folded_threads.contains(&entry.id) {
            out.rows.push(Row::Stub {
                entry: index,
                summary: entry.summary.clone(),
                selected,
                dim,
            });
            return;
        }
        out.rows.push(Row::Header {
            entry: index,
            summary: entry.summary.clone(),
            selected,
            dim,
        });
        let selected_message =
            selected.then(|| self.thread_cursor().message().min(thread.replies().len()));
        let layouts = self
            .message_layout_cache
            .layout(thread, body_width, self.highlighter());
        // Every author as `author_label` names them (ADR 0058, ADR
        // 0061): the configured name for the user, `name (type)` for an
        // agent, the comment's the same as a reply's.
        let mut message =
            |message: usize, author: &Author, created: u64, badge: Option<&'static str>| {
                let message_selected = selected_message == Some(message);
                out.rows.push(Row::Message {
                    entry: index,
                    message,
                    user: author.is_user(),
                    author: author_label(author, user),
                    created,
                    badge,
                    dim,
                    selected: message_selected,
                });
                // The body as the expanded thread in the file draws it (ADR
                // 0037): Markdown, a newline kept as a line break, fences
                // coloured by the app's highlighter.
                for line in layouts.body(message).lines() {
                    out.rows.push(Row::Body {
                        entry: index,
                        message,
                        user: author.is_user(),
                        line: line.clone(),
                        dim,
                        selected: message_selected,
                    });
                }
            };
        message(0, thread.author(), thread.created(), None);
        for (reply_index, reply) in thread.replies().iter().enumerate() {
            let badge = reply.proposes_resolution().then_some("proposes resolving");
            message(reply_index + 1, reply.author(), reply.created(), badge);
        }
        if let Some(evidence) = &entry.evidence {
            for line in Layout::render_message(evidence, body_width, self.highlighter()).lines() {
                out.rows.push(Row::Evidence {
                    entry: index,
                    line: line.clone(),
                    dim: false,
                    selected: false,
                });
            }
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
    pub(super) fn selected_index(&self, rows: &Rows) -> Option<usize> {
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
        self.review_list.on_file = false;
        self.set_thread_cursor(entry.id.clone());
        self.scroll_to_selection(rows, index);
    }

    fn select_message(&mut self, rows: &Rows, entry: usize, message: usize) {
        let Some(id) = rows.entries.get(entry).map(|entry| entry.id.clone()) else {
            return;
        };
        self.review_list.on_file = false;
        self.set_thread_cursor_message(id, message);
        self.scroll_to_selection(rows, entry);
    }

    /// Rest the cursor on `path`'s file row (ADR 0076): the thread
    /// cursor goes to the file's first thread and the row is kept on
    /// screen.
    pub(super) fn rest_on_file(&mut self, rows: &Rows, path: &Path) {
        let Some(entry) = rows.first_entry_of(path) else {
            return;
        };
        self.select_entry(rows, entry);
        self.review_list.on_file = true;
        self.scroll_to_selection(rows, entry);
    }

    /// Land on `stop`: a file row, or a thread.
    fn land_on_stop(&mut self, rows: &Rows, stop: &Stop) {
        match stop {
            Stop::File(path) => self.rest_on_file(rows, path),
            Stop::Entry(entry) => self.select_entry(rows, *entry),
        }
    }

    /// The stop the cursor is on: its file's row while it rests there
    /// or the file is folded over it, else its thread.
    pub(super) fn cursor_stop(&self, rows: &Rows, index: usize) -> Stop {
        let path = rows.entries[index].path.clone();
        let file_row = rows.row_of_stop(&Stop::File(path.clone())).is_some();
        if file_row && (self.review_list.on_file || rows.entry_row(index).is_none()) {
            Stop::File(path)
        } else {
            Stop::Entry(index)
        }
    }

    /// Whether the cursor is on a row that shows no messages (ADR
    /// 0076): a file row or a folded thread, where `l`/`h` have nothing
    /// to walk.
    pub(crate) fn review_cursor_folded(&self) -> bool {
        if !self.review_list.is_open() {
            return false;
        }

        let rows = self.review_rows(self.column_width());
        let Some(index) = self.selected_index(&rows) else {
            return false;
        };
        match self.cursor_stop(&rows, index) {
            Stop::File(_) => true,
            Stop::Entry(entry) => rows
                .entries
                .get(entry)
                .is_some_and(|entry| self.review_list.folded_threads.contains(&entry.id)),
        }
    }

    /// Whether the review cursor rests on a thread rather than its file row.
    pub(crate) fn review_cursor_on_thread(&self) -> bool {
        if !self.review_list.is_open() {
            return false;
        }
        let rows = self.review_rows(self.column_width());
        self.selected_index(&rows)
            .is_some_and(|index| matches!(self.cursor_stop(&rows, index), Stop::Entry(_)))
    }

    /// `l` / `h`: the next or previous message of the cursor's thread,
    /// when its messages are shown.
    pub(crate) fn review_message_step(&mut self, delta: isize) {
        if self.review_cursor_folded() {
            return;
        }
        self.message_step(delta);
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
        let range = match self.cursor_stop(rows, entry) {
            Stop::File(_) => rows.file_row_of(entry).map(|row| row..row + 1),
            Stop::Entry(_) => rows
                .message_range(entry, self.thread_cursor().message())
                .or_else(|| rows.entry_row(entry).map(|row| row..row + 1)),
        };
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

    /// `j` / `k`: move by `delta` stops in the list's order (ADR 0066,
    /// ADR 0076): file rows, and each thread of an unfolded file.
    pub(crate) fn review_step(&mut self, delta: isize) {
        let rows = self.review_rows(self.column_width());
        let Some(index) = self.selected_index(&rows) else {
            return;
        };
        let stops = rows.stops();
        if stops.is_empty() {
            return;
        }
        let stop = self.cursor_stop(&rows, index);
        let at = stops.iter().position(|s| *s == stop).unwrap_or(0);
        let last = stops.len() - 1;
        let target = stops[at.saturating_add_signed(delta).min(last)].clone();
        self.land_on_stop(&rows, &target);
    }

    /// `Ctrl-d` / `Ctrl-u`: the message half a page of rows below or
    /// above the highlighted one, the nearest when that row is a heading.
    pub(crate) fn review_page(&mut self, direction: isize) {
        let rows = self.review_rows(self.column_width());
        let Some(entry) = self.selected_index(&rows) else {
            return;
        };
        let Some(start) = (match self.cursor_stop(&rows, entry) {
            Stop::File(_) => rows.file_row_of(entry),
            Stop::Entry(_) => rows
                .message_range(entry, self.thread_cursor().message())
                .map(|range| range.start)
                .or_else(|| rows.entry_row(entry)),
        }) else {
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
            (target..=last).find_map(|row| rows.landing_at(row))
        } else {
            (0..=target).rev().find_map(|row| rows.landing_at(row))
        };
        match landing {
            Some(Landing::Message(entry, message)) => self.select_message(&rows, entry, message),
            Some(Landing::Stop(stop)) => self.land_on_stop(&rows, &stop),
            None => {}
        }
    }

    /// `gg` / `G`: the first stop, or the last.
    pub(crate) fn review_goto(&mut self, end: bool) {
        let rows = self.review_rows(self.column_width());
        let stops = rows.stops();
        let target = if end { stops.last() } else { stops.first() };
        if let Some(target) = target.cloned() {
            self.land_on_stop(&rows, &target);
        }
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

    /// A click on list row `row` (below the header) selects its message
    /// or its thread; on a file row it folds or unfolds the file and
    /// rests the cursor on the row (ADR 0066, ADR 0076).
    pub(crate) fn review_click(&mut self, row: usize) {
        let rows = self.review_rows(self.column_width());
        let at = self.review_list.scroll + row;
        match rows.landing_at(at) {
            Some(Landing::Stop(Stop::File(path))) => self.review_toggle_fold(&path),
            Some(Landing::Stop(Stop::Entry(entry))) => self.select_entry(&rows, entry),
            Some(Landing::Message(entry, message)) => self.select_message(&rows, entry, message),
            None => {}
        }
        self.focus = Focus::Review;
    }

    /// A right-click on list row `row` (ADR 0066): on a file row the
    /// cursor rests on the row and the path is returned for the file's
    /// menu; elsewhere the click selects as a left one.
    pub(crate) fn review_point(&mut self, row: usize) -> Option<PathBuf> {
        let rows = self.review_rows(self.column_width());
        let at = self.review_list.scroll + row;
        let path = rows.file_at(at).map(Path::to_path_buf)?;
        self.rest_on_file(&rows, &path);
        self.focus = Focus::Review;
        Some(path)
    }

    /// `f`: narrow to the current file, or widen again.
    pub(crate) fn review_toggle_file(&mut self) {
        if self.review.view != ReviewView::Board {
            return;
        }
        self.review.file_only = !self.review.file_only;
        self.reshow_review();
    }

    /// `x` in the list or the threads pane: list resolved threads too,
    /// or hide them again (ADR 0049).
    pub(crate) fn review_toggle_resolved(&mut self) {
        if self.review.view != ReviewView::Board {
            return;
        }
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
    pub(super) fn reshow_review(&mut self) {
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
