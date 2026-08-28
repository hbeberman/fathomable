// @okf-doc: /decisions/0025-thread-list.md
//! The thread list (ADR 0025): every thread on the current work, open then
//! resolved, grouped by file, drawn in place of the document.
//!
//! The list keeps only its own state — whether it is open, the file
//! filter, the selected thread, the scroll, and the folds. Its rows are
//! computed from the store on every draw and key by [`App::thread_list_rows`],
//! so a reload or a scope change needs nothing invalidated.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use fathomable_core::annotations::{LineRange, Status, Thread, ThreadId};
use fathomable_core::layout::wrap_text;

use super::threads::{ComposeTarget, MarkKind};
use super::{App, Focus};

/// Rows kept visible above and below the selected entry.
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
    selected: Option<ThreadId>,
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
    kind: MarkKind,
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
        kind: MarkKind,
        updated: u64,
        selected: bool,
        folded: bool,
        dim: bool,
    },
    /// `author  age  [badge]`.
    Message {
        author: String,
        created: u64,
        badge: Option<&'static str>,
        dim: bool,
    },
    /// One wrapped line of a comment or reply, already indented.
    Body {
        text: String,
        dim: bool,
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

    /// The entry whose rows include `row`.
    pub fn entry_at(&self, row: usize) -> Option<usize> {
        self.rows[..self.rows.len().min(row + 1)]
            .iter()
            .rev()
            .find_map(|r| match r {
                Row::Header { entry, .. } => Some(*entry),
                _ => None,
            })
    }
}

fn is_open(kind: MarkKind) -> bool {
    matches!(
        kind,
        MarkKind::Open | MarkKind::Waiting | MarkKind::Edited | MarkKind::Detached
    )
}

fn kind_of(thread: &Thread) -> MarkKind {
    if thread.awaits_user() {
        return MarkKind::Waiting;
    }
    match thread.status() {
        Status::Open => MarkKind::Open,
        Status::Resolved => MarkKind::Resolved,
        Status::AutoResolved => MarkKind::AutoResolved,
    }
}

impl App {
    /// The list, open or not.
    pub fn thread_list(&self) -> &ThreadList {
        &self.list
    }

    /// `Space A`: show the list in place of the document. The pane closes;
    /// the selection, filter, and folds are whatever they were last time.
    pub fn open_thread_list(&mut self) {
        if self.store.is_none() {
            self.store_mut();
            return;
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
            if !self.scope.includes(thread) {
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
    fn placement_of(&self, thread: &Thread) -> (LineRange, MarkKind) {
        self.docs
            .iter()
            .find(|doc| doc.relative == thread.path())
            .and_then(|doc| doc.marks.iter().find(|mark| mark.id() == thread.id()))
            .map_or_else(
                || (thread.range(), kind_of(thread)),
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
                let selected = self.list.selected.as_ref() == Some(&entry.id);
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
            selected,
            folded,
            dim,
        });
        if folded {
            return;
        }
        let mut message = |author: &str, created: u64, body: &str, badge: Option<&'static str>| {
            out.rows.push(Row::Message {
                author: author.to_owned(),
                created,
                badge,
                dim,
            });
            for paragraph in body.lines() {
                for line in wrap_text(paragraph, body_width) {
                    out.rows.push(Row::Body {
                        text: format!("{}{line}", " ".repeat(MESSAGE_INDENT)),
                        dim,
                    });
                }
            }
        };
        message("user", thread.created(), thread.comment(), None);
        for reply in thread.replies() {
            let badge = reply.proposes_resolution().then_some("proposes resolving");
            message(reply.author().name(), reply.created(), reply.body(), badge);
        }
        out.rows.push(Row::Blank);
    }

    /// Rows the list has for its entries: the column minus its header.
    fn list_rows(&self) -> usize {
        self.text_rows().saturating_sub(1).max(1)
    }

    /// The selected entry's index, or the first entry when the selection
    /// is gone or unset.
    fn selected_index(&self, rows: &Rows) -> Option<usize> {
        let by_id = self
            .list
            .selected
            .as_ref()
            .and_then(|id| rows.entries.iter().position(|entry| &entry.id == id));
        by_id.or_else(|| (!rows.entries.is_empty()).then_some(0))
    }

    /// Select entry `index` and scroll so its header is on screen.
    fn select_entry(&mut self, rows: &Rows, index: usize) {
        let Some(entry) = rows.entries.get(index) else {
            return;
        };
        self.list.selected = Some(entry.id.clone());
        let Some(header) = rows.header_row(index) else {
            return;
        };
        let visible = self.list_rows();
        let top = header.saturating_sub(SCROLLOFF);
        let bottom = (header + SCROLLOFF + 1).min(rows.rows.len());
        if top < self.list.scroll {
            self.list.scroll = top;
        } else if bottom > self.list.scroll + visible {
            self.list.scroll = bottom.saturating_sub(visible);
        }
    }

    /// `j`/`k` and the half-page keys: move the selection by `delta`.
    pub fn thread_list_move(&mut self, delta: isize) {
        let rows = self.thread_list_rows(self.column_width());
        let Some(index) = self.selected_index(&rows) else {
            return;
        };
        let last = rows.entries.len().saturating_sub(1);
        let target = index.saturating_add_signed(delta).min(last);
        self.select_entry(&rows, target);
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

    /// A click on list row `row` (below the header) selects its entry.
    pub fn thread_list_click(&mut self, row: usize) {
        let rows = self.thread_list_rows(self.column_width());
        if let Some(index) = rows.entry_at(self.list.scroll + row) {
            self.select_entry(&rows, index);
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
        self.list.selected = Some(id);
    }

    /// `Z`: fold or unfold the resolved section.
    pub fn thread_list_fold_resolved(&mut self) {
        self.list.resolved_folded = !self.list.resolved_folded;
        let rows = self.thread_list_rows(self.column_width());
        if let Some(index) = self.selected_index(&rows) {
            self.select_entry(&rows, index);
        }
    }

    /// The selected entry, when there is one.
    fn selected_entry(&self) -> Option<Entry> {
        let rows = self.thread_list_rows(self.column_width());
        let index = self.selected_index(&rows)?;
        rows.entries.get(index).cloned()
    }

    /// Enter: open the entry's file at the thread, open the pane on it,
    /// and close the list.
    pub fn thread_list_open_entry(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        self.close_thread_list();
        self.open(Path::new(&entry.path));
        if self.current_path() == entry.path {
            self.show_thread(entry.id);
        }
    }

    /// `r`: reply to the selected entry through the comment box.
    pub fn thread_list_reply(&mut self) {
        if let Some(entry) = self.selected_entry() {
            self.open_compose(ComposeTarget::Reply(entry.id));
        }
    }

    /// `x`: resolve the selected entry, or reopen it.
    pub fn thread_list_toggle_resolved(&mut self) {
        if let Some(entry) = self.selected_entry() {
            self.toggle_resolved(&entry.id);
            let rows = self.thread_list_rows(self.column_width());
            if let Some(index) = self.selected_index(&rows) {
                self.select_entry(&rows, index);
            }
        }
    }
}
