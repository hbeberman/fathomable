// @okf-doc: /decisions/0025-thread-list.md
//! The thread list (ADR 0025): every thread on the current work, open then
//! resolved, grouped by file, drawn in place of the document.
//!
//! The list keeps only its own state — whether it is open, the file
//! filter, the selected thread and message, the scroll, and the folds. Its rows are
//! computed from the store on every draw and key by [`App::thread_list_rows`],
//! so a reload or a scope change needs nothing invalidated.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use fathomable_core::annotations::{LineRange, Thread, ThreadId};
use fathomable_core::layout::wrap_text;

use super::threads::ThreadState;
use super::{App, Focus};

/// Rows kept visible above and below the selected message.
const SCROLLOFF: usize = 2;
/// Cells a message body is indented from the column edge: two deeper than
/// its author row. Body rows carry the indent in their text, so the UI
/// draws them verbatim and the wrap width already accounts for it.
const MESSAGE_INDENT: usize = 5;

/// The list's state; the rows are derived from the store.
#[derive(Debug, Default)]
pub struct ThreadList {
    open: bool,
    /// Only the current document's threads.
    file_only: bool,
    scroll: usize,
    folded: HashSet<ThreadId>,
    resolved_folded: bool,
}

impl ThreadList {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Whether the list is narrowed to the current document.
    pub fn file_only(&self) -> bool {
        self.file_only
    }

    /// A document took the column back.
    pub(super) fn close(&mut self) {
        self.open = false;
    }
}

/// One thread in list order, for moving and acting on the selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    id: ThreadId,
    path: PathBuf,
    range: LineRange,
    kind: ThreadState,
}

/// One drawn row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// `open 4` / `resolved 7`.
    Section {
        resolved: bool,
        count: usize,
        folded: bool,
    },
    File(PathBuf),
    /// The first row of an entry: range, status, age.
    Header {
        entry: usize,
        range: LineRange,
        kind: ThreadState,
        updated: u64,
        selected: bool,
        folded: bool,
        dim: bool,
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
    /// One wrapped line of a comment or reply, already indented.
    Body {
        entry: usize,
        message: usize,
        text: String,
        dim: bool,
        selected: bool,
    },
    Blank,
}

/// The computed list: rows to draw and entries to act on.
#[derive(Debug, Default)]
pub struct Rows {
    pub rows: Vec<Row>,
    pub entries: Vec<Entry>,
}

impl Rows {
    /// The row index of `entry`'s header.
    pub fn header_row(&self, entry: usize) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(row, Row::Header { entry: e, .. } if *e == entry))
    }

    /// The entry and optional message under `row`.
    pub fn selection_at(&self, row: usize) -> Option<(usize, Option<usize>)> {
        match self.rows.get(row)? {
            Row::Header { entry, .. } => Some((*entry, None)),
            Row::Message { entry, message, .. } | Row::Body { entry, message, .. } => {
                Some((*entry, Some(*message)))
            }
            Row::Section { .. } | Row::File(_) | Row::Blank => None,
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
}

fn is_open(kind: ThreadState) -> bool {
    matches!(kind, ThreadState::Open | ThreadState::Waiting)
}

impl App {
    /// The list, open or not.
    pub fn thread_list(&self) -> &ThreadList {
        &self.list
    }

    /// `Space A`: show the list in place of the document, or focus it
    /// when it is open, or close it when it is open and focused.
    pub fn toggle_thread_list(&mut self) {
        if self.list.is_open() && self.focus == Focus::Threads {
            self.close_thread_list();
        } else if self.list.is_open() {
            self.focus = Focus::Threads;
        } else {
            self.open_thread_list();
        }
    }

    /// Show the list in place of the document. The pane closes; the
    /// filter and folds are whatever they were last time, and the cursor
    /// is where the reader was (ADR 0046).
    pub fn open_thread_list(&mut self) {
        if self.store.is_none() {
            self.store_mut();
            return;
        }
        // The cursor the pane or the text was on becomes the list's.
        let cursor = self.thread_cursor();
        if let Some(id) = cursor.thread().cloned() {
            self.set_thread_cursor_message(id, cursor.message());
        }
        self.thread = None;
        self.list.open = true;
        self.focus = Focus::Threads;
        let rows = self.thread_list_rows(self.column_width());
        if rows.entries.is_empty() {
            self.notice(if self.list.file_only {
                "no threads in this file"
            } else {
                "no threads on this work"
            });
        } else if let Some(index) = self.selected_index(&rows) {
            self.select_entry(&rows, index);
        }
        self.relayout();
    }

    /// Esc: back to the document that was showing.
    pub fn close_thread_list(&mut self) {
        if !self.list.open {
            return;
        }
        self.list.open = false;
        if self.focus == Focus::Threads {
            self.focus = Focus::View;
        }
        self.relayout();
    }

    /// Columns the list has: the text column without the tree.
    fn column_width(&self) -> usize {
        self.width.saturating_sub(self.sidebar_width()).max(1)
    }

    /// The rows and entries for a list `width` cells wide: threads the
    /// scope shows (or the current file's), open before resolved, files
    /// in path order, threads in line order.
    pub fn thread_list_rows(&self, width: usize) -> Rows {
        let Some(store) = self.store.as_ref() else {
            return Rows::default();
        };
        let current = self.current_path();
        let mut open: BTreeMap<PathBuf, Vec<Entry>> = BTreeMap::new();
        let mut resolved: BTreeMap<PathBuf, Vec<Entry>> = BTreeMap::new();
        for thread in store.threads() {
            if !self.reach.includes(thread) {
                continue;
            }
            if self.list.file_only && thread.path() != current {
                continue;
            }
            let (range, kind) = self.placement_of(thread);
            let entry = Entry {
                id: thread.id().clone(),
                path: thread.path().to_path_buf(),
                range,
                kind,
            };
            let section = if is_open(kind) {
                &mut open
            } else {
                &mut resolved
            };
            section.entry(entry.path.clone()).or_default().push(entry);
        }
        let mut out = Rows::default();
        self.push_section(&mut out, open, width, false);
        self.push_section(&mut out, resolved, width, true);
        out
    }

    /// Range and status of `thread`: from the loaded document's mark when
    /// its file is open this session, else as stored.
    fn placement_of(&self, thread: &Thread) -> (LineRange, ThreadState) {
        self.docs
            .iter()
            .find(|doc| doc.relative == thread.path())
            .and_then(|doc| doc.marks.iter().find(|mark| mark.id() == thread.id()))
            .map_or_else(
                || (thread.range(), ThreadState::of(thread)),
                |mark| (mark.range(), mark.kind()),
            )
    }

    fn push_section(
        &self,
        out: &mut Rows,
        files: BTreeMap<PathBuf, Vec<Entry>>,
        width: usize,
        resolved: bool,
    ) {
        let count: usize = files.values().map(Vec::len).sum();
        if count == 0 {
            return;
        }
        let folded = resolved && self.list.resolved_folded;
        let cursor = self.thread_cursor();
        if !out.rows.is_empty() {
            out.rows.push(Row::Blank);
        }
        out.rows.push(Row::Section {
            resolved,
            count,
            folded,
        });
        let body_width = width.saturating_sub(MESSAGE_INDENT).max(1);
        for (path, mut entries) in files {
            entries.sort_by_key(|entry| entry.range.start());
            if !folded {
                out.rows.push(Row::File(path));
            }
            for entry in entries {
                let index = out.entries.len();
                let selected = cursor.thread() == Some(&entry.id);
                let entry_folded = self.list.folded.contains(&entry.id);
                if !folded {
                    self.push_entry(
                        out,
                        &entry,
                        index,
                        selected,
                        entry_folded,
                        resolved,
                        body_width,
                    );
                }
                out.entries.push(entry);
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one row builder, all inputs local"
    )]
    fn push_entry(
        &self,
        out: &mut Rows,
        entry: &Entry,
        index: usize,
        selected: bool,
        folded: bool,
        dim: bool,
        body_width: usize,
    ) {
        let Some(thread) = self.thread(&entry.id) else {
            return;
        };
        out.rows.push(Row::Header {
            entry: index,
            range: entry.range,
            kind: entry.kind,
            updated: thread.updated(),
            selected: selected && folded,
            folded,
            dim,
        });
        if folded {
            return;
        }
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
            for paragraph in body.lines() {
                for line in wrap_text(paragraph, body_width) {
                    out.rows.push(Row::Body {
                        entry: index,
                        message,
                        text: format!("{}{line}", " ".repeat(MESSAGE_INDENT)),
                        dim,
                        selected: message_selected,
                    });
                }
            }
        };
        message(0, "user", thread.created(), thread.comment(), None);
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

    /// Rows the list has for its entries: the column minus its header.
    fn list_rows(&self) -> usize {
        self.text_rows().saturating_sub(1).max(1)
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

    /// Scroll enough to keep the cursor's message, or a folded header,
    /// visible; the entry is re-found when the rows changed under it.
    pub(super) fn thread_list_follow_cursor(&mut self) {
        let rows = self.thread_list_rows(self.column_width());
        if let Some(index) = self.selected_index(&rows) {
            self.select_entry(&rows, index);
        }
    }

    /// Scroll enough to keep the selected message, or a folded header, visible.
    fn scroll_to_selection(&mut self, rows: &Rows, entry: usize) {
        let range = rows
            .message_range(entry, self.thread_cursor().message())
            .or_else(|| rows.header_row(entry).map(|row| row..row + 1));
        let Some(range) = range else {
            return;
        };
        let visible = self.list_rows();
        let top = range.start.saturating_sub(SCROLLOFF);
        let bottom = (range.end + SCROLLOFF).min(rows.rows.len());
        if top < self.list.scroll {
            self.list.scroll = top;
        } else if bottom > self.list.scroll + visible {
            self.list.scroll = if range.len() > visible {
                range.start
            } else {
                bottom.saturating_sub(visible)
            };
        }
    }

    /// `h` / `l`: move between threads by `delta` in the list's order.
    pub fn thread_list_step(&mut self, delta: isize) {
        let rows = self.thread_list_rows(self.column_width());
        let Some(index) = self.selected_index(&rows) else {
            return;
        };
        let last = rows.entries.len().saturating_sub(1);
        let target = index.saturating_add_signed(delta).min(last);
        self.select_entry(&rows, target);
    }

    /// `Ctrl-d` / `Ctrl-u`: the message half a page of rows below or
    /// above the highlighted one, the nearest when that row is a heading.
    pub fn thread_list_page(&mut self, direction: isize) {
        let rows = self.thread_list_rows(self.column_width());
        let Some(entry) = self.selected_index(&rows) else {
            return;
        };
        let Some(start) = rows
            .message_range(entry, self.thread_cursor().message())
            .map(|range| range.start)
            .or_else(|| rows.header_row(entry))
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
    pub fn thread_list_goto(&mut self, end: bool) {
        let rows = self.thread_list_rows(self.column_width());
        let target = if end {
            rows.entries.len().saturating_sub(1)
        } else {
            0
        };
        self.select_entry(&rows, target);
        if !end {
            self.list.scroll = 0;
        }
    }

    /// The wheel: scroll the rows; the selection stays where it is.
    pub fn thread_list_scroll(&mut self, delta: isize) {
        let rows = self.thread_list_rows(self.column_width());
        let max = rows.rows.len().saturating_sub(self.list_rows());
        self.list.scroll = self.list.scroll.saturating_add_signed(delta).min(max);
    }

    /// A click on list row `row` (below the header) selects its message.
    pub fn thread_list_click(&mut self, row: usize) {
        let rows = self.thread_list_rows(self.column_width());
        if let Some((entry, message)) = rows.selection_at(self.list.scroll + row) {
            if let Some(message) = message {
                self.select_message(&rows, entry, message);
            } else {
                self.select_entry(&rows, entry);
            }
        }
        self.focus = Focus::Threads;
    }

    /// `f`: narrow to the current file, or widen again.
    pub fn thread_list_toggle_file(&mut self) {
        self.list.file_only = !self.list.file_only;
        self.list.scroll = 0;
        let rows = self.thread_list_rows(self.column_width());
        if rows.entries.is_empty() {
            self.notice(if self.list.file_only {
                "no threads in this file"
            } else {
                "no threads on this work"
            });
        } else if let Some(index) = self.selected_index(&rows) {
            self.select_entry(&rows, index);
        }
    }

    /// `z`: fold the selected entry to its header, or unfold it.
    pub fn thread_list_fold(&mut self) {
        let rows = self.thread_list_rows(self.column_width());
        let Some(index) = self.selected_index(&rows) else {
            return;
        };
        let id = rows.entries[index].id.clone();
        if !self.list.folded.remove(&id) {
            self.list.folded.insert(id.clone());
        }
        self.set_thread_cursor(id);
        let rows = self.thread_list_rows(self.column_width());
        if let Some(index) = self.selected_index(&rows) {
            self.select_entry(&rows, index);
        }
    }

    /// `Z`: fold or unfold the resolved section.
    pub fn thread_list_fold_resolved(&mut self) {
        self.list.resolved_folded = !self.list.resolved_folded;
        let rows = self.thread_list_rows(self.column_width());
        if let Some(index) = self.selected_index(&rows) {
            self.select_entry(&rows, index);
        }
    }

    /// The selected entry's index, for a caller about to change the store.
    pub(super) fn thread_list_selected_index(&self) -> Option<usize> {
        if !self.list.is_open() {
            return None;
        }
        self.selected_index(&self.thread_list_rows(self.column_width()))
    }

    /// Re-select after the store changed under the list: the entry with
    /// the selected id, else the one now at `place` (the index before
    /// the change), else the last, else nothing.
    pub(super) fn thread_list_reselect(&mut self, place: Option<usize>) {
        if !self.list.is_open() {
            return;
        }
        let rows = self.thread_list_rows(self.column_width());
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
