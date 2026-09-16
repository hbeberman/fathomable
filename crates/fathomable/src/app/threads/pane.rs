// @okf-doc: /decisions/0027-revisiting-threads.md
//! The threads pane (ADR 0027, reshaped by ADR 0049 and ADR 0066): the
//! lower pane of the sidebar, listing this file's threads in line order
//! or the whole workspace's grouped by file, two rows per thread,
//! resolved ones hidden until asked for.
//!
//! The pane keeps no selection of its own. The highlighted entry is the
//! thread cursor's (ADR 0046), derived on every draw and key from the
//! marks and the store, so the pane can never disagree with the text or
//! go stale on a reload. Its state is whether it is shown, its scope,
//! the files it has folded, and the split a drag gave it; the resolved
//! flag is the review's, shared with the review list.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use fathomable_core::annotations::{LineRange, ThreadId};

use crate::app::threads::author_label;
use crate::app::threads::words::Words;
use crate::app::threads::{Mark, ThreadState};
use crate::app::{App, Focus};

/// Rows the pane needs before its entries: the rule and the header.
const CHROME_ROWS: usize = 2;
/// The fewest rows the pane is drawn with: rule, header, one entry row.
const MIN_ROWS: usize = 3;
/// Rows the tree keeps above the pane when both are shown.
const TREE_MIN_ROWS: usize = 2;

/// Which threads the pane lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PaneScope {
    /// The current document's threads in line order.
    #[default]
    File,
    /// Every thread on the work, files in the files pane's order,
    /// threads by line, under a row per file.
    Workspace,
}

impl PaneScope {
    /// The word the header shows.
    #[must_use]
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Workspace => "workspace",
        }
    }
}

/// One thread of the pane, ready to draw as two rows (ADR 0066).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaneEntry {
    id: ThreadId,
    path: PathBuf,
    /// `None` for a thread on the file as a whole (ADR 0063).
    range: Option<LineRange>,
    words: Words,
    /// Who wrote the newest message, as `name (role)`.
    author: String,
    /// The same author's name without the optional role.
    author_name: String,
    /// The first line of the newest message.
    summary: String,
    replies: usize,
    /// When the thread last changed.
    updated: u64,
    /// The thread is on the current document.
    current: bool,
    /// The thread cursor's thread.
    selected: bool,
    /// The branch of the worktree showing it (ADR 0070), or the commit
    /// a past thread was resolved at (ADR 0072).
    note: Option<String>,
}

impl PaneEntry {
    /// After the author: the branch when another worktree shows the
    /// thread (ADR 0070), the commit when it is resolved at an earlier
    /// one of this branch (ADR 0072).
    pub(crate) fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn range(&self) -> Option<LineRange> {
        self.range
    }

    pub(crate) fn kind(&self) -> ThreadState {
        self.words.state()
    }

    /// Placement and state words, and the circle (ADR 0032, ADR 0066).
    pub(crate) fn words(&self) -> Words {
        self.words
    }

    pub(crate) fn author(&self) -> &str {
        &self.author
    }

    pub(crate) fn author_name(&self) -> &str {
        &self.author_name
    }

    pub(crate) fn summary(&self) -> &str {
        &self.summary
    }

    pub(crate) fn replies(&self) -> usize {
        self.replies
    }

    pub(crate) fn updated(&self) -> u64 {
        self.updated
    }

    pub(crate) fn current(&self) -> bool {
        self.current
    }

    pub(crate) fn selected(&self) -> bool {
        self.selected
    }

    /// Where the thread is in its file: `L9-11`, or `file` for a thread
    /// on the file as a whole (ADR 0063). The file itself is on the
    /// group row (ADR 0066).
    #[must_use]
    pub(crate) fn place(&self) -> String {
        self.range
            .map_or_else(|| "file".to_owned(), |range| format!("L{range}"))
    }
}

/// One entry of the pane: a file's row over its threads, or a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PaneRow {
    /// A file with `count` listed threads (ADR 0066); `selected` when
    /// the cursor's thread is inside it while it is folded.
    File {
        path: PathBuf,
        count: usize,
        folded: bool,
        current: bool,
        selected: bool,
    },
    Thread(PaneEntry),
}

/// One screen row of the pane's body, pointing at its entry in the
/// rows: a file row, or the first or second row of a thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneLine {
    File(usize),
    First(usize),
    Second(usize),
}

impl PaneLine {
    fn row(self) -> usize {
        match self {
            Self::File(row) | Self::First(row) | Self::Second(row) => row,
        }
    }
}

/// The screen rows `rows` take: one per file row, two per thread.
pub(crate) fn pane_lines(rows: &[PaneRow]) -> Vec<PaneLine> {
    rows.iter()
        .enumerate()
        .flat_map(|(index, row)| match row {
            PaneRow::File { .. } => vec![PaneLine::File(index)],
            PaneRow::Thread(_) => vec![PaneLine::First(index), PaneLine::Second(index)],
        })
        .collect()
}

/// What a right-click on the pane landed on (ADR 0066).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PanePoint {
    /// A file row; the cursor is on the file's first thread.
    File(PathBuf),
    /// A thread; the cursor is on it.
    Thread,
}

impl App {
    /// Whether the pane is shown in the sidebar.
    pub(crate) fn threads_pane_shown(&self) -> bool {
        self.sidebar.threads
    }

    /// Which threads the pane lists.
    pub(crate) fn sidebar_scope(&self) -> PaneScope {
        self.sidebar.scope
    }

    /// Whether the pane has folded `path` to its row (ADR 0066).
    pub(crate) fn threads_pane_is_folded(&self, path: &Path) -> bool {
        self.sidebar.folded.contains(path)
    }

    /// The threads the pane lists, in its order, resolved ones only when
    /// the review shows them: this file's by line, or the workspace's by
    /// file and line (ADR 0066).
    #[cfg(test)]
    pub(crate) fn threads_pane_ids(&self) -> Vec<ThreadId> {
        self.review_entries(self.sidebar.scope == PaneScope::File)
            .into_iter()
            .map(|entry| entry.id().clone())
            .collect()
    }

    /// The threads `j` / `k` and the wheel stop on: every listed thread,
    /// a folded file counting once through its first thread (ADR 0066).
    fn threads_pane_stops(&self) -> Vec<ThreadId> {
        let mut last_path: Option<PathBuf> = None;
        let mut out = Vec::new();
        for entry in self.review_entries(self.sidebar.scope == PaneScope::File) {
            let folded = self.sidebar.scope == PaneScope::Workspace
                && self.sidebar.folded.contains(entry.path());
            let first_of_file = last_path.as_deref() != Some(entry.path());
            if !folded || first_of_file {
                out.push(entry.id().clone());
            }
            last_path = Some(entry.path().to_path_buf());
        }
        out
    }

    /// The pane's entries, ready to draw: in workspace scope a row per
    /// file over its threads, a folded file its row alone; in file scope
    /// the threads alone.
    pub(crate) fn threads_pane_rows(&self) -> Vec<PaneRow> {
        let grouped = self.sidebar.scope == PaneScope::Workspace;
        let cursor = self.thread_cursor().thread().cloned();
        let entries = self.review_entries(!grouped);
        let mut out = Vec::new();
        let mut index = 0;
        while index < entries.len() {
            let path = entries[index].path().to_path_buf();
            let group_end = entries[index..]
                .iter()
                .position(|entry| entry.path() != path)
                .map_or(entries.len(), |len| index + len);
            let folded = grouped && self.sidebar.folded.contains(&path);
            if grouped {
                let selected = folded
                    && entries[index..group_end]
                        .iter()
                        .any(|entry| cursor.as_ref() == Some(entry.id()));
                out.push(PaneRow::File {
                    path: path.clone(),
                    count: group_end - index,
                    folded,
                    current: path == self.current_path(),
                    selected,
                });
            }
            if !folded {
                for entry in &entries[index..group_end] {
                    let Some(thread) = self.thread(entry.id()) else {
                        continue;
                    };
                    let (author, body) = thread
                        .replies()
                        .last()
                        .map_or((thread.author(), thread.comment()), |reply| {
                            (reply.author(), reply.body())
                        });
                    out.push(PaneRow::Thread(PaneEntry {
                        note: entry.note().map(str::to_owned),
                        id: entry.id().clone(),
                        path: path.clone(),
                        range: entry.range(),
                        words: entry.words(),
                        author: author_label(author, self.user_name()),
                        author_name: if author.is_user() {
                            self.user_name()
                        } else {
                            author.name()
                        }
                        .to_owned(),
                        summary: body.lines().next().unwrap_or("").to_owned(),
                        replies: thread.replies().len(),
                        updated: thread.updated(),
                        current: path == self.current_path(),
                        selected: cursor.as_ref() == Some(entry.id()),
                    }));
                }
            }
            index = group_end;
        }
        out
    }

    /// The pane's threads alone, in its order, for tests that read one
    /// thread's facts.
    #[cfg(test)]
    pub(crate) fn threads_pane_entries(&self) -> Vec<PaneEntry> {
        self.threads_pane_rows()
            .into_iter()
            .filter_map(|row| match row {
                PaneRow::Thread(entry) => Some(entry),
                PaneRow::File { .. } => None,
            })
            .collect()
    }

    /// Rows the pane takes at the bottom of the sidebar: 0 when hidden, the
    /// whole column when the tree is hidden, else the split `sidebar.split`
    /// or a drag set, kept between one entry and the tree's minimum.
    pub(crate) fn threads_pane_height(&self) -> usize {
        if !self.sidebar.threads {
            return 0;
        }
        let rows = self.pane_rows();
        if self.tree().is_none() {
            return rows;
        }
        let tallest = rows.saturating_sub(TREE_MIN_ROWS);
        let least = MIN_ROWS.min(tallest);
        self.sidebar
            .split
            .unwrap_or(self.sidebar.config.split)
            .clamp(least, tallest)
    }

    /// Rows the pane's entries have: the height less the rule and the
    /// header, and less the key bar while the pane has the keys (ADR
    /// 0066).
    pub(crate) fn threads_pane_body_rows(&self) -> usize {
        self.threads_pane_height()
            .saturating_sub(CHROME_ROWS)
            .saturating_sub(usize::from(self.focus == Focus::ThreadsPane))
    }

    /// Rows the files pane has, 0 when it is hidden.
    pub(crate) fn tree_rows(&self) -> usize {
        if self.tree().is_none() {
            return 0;
        }
        self.pane_rows()
            .saturating_sub(self.threads_pane_height())
            .max(1)
    }

    /// The highlighted entry: the thread cursor's thread among the listed
    /// ones (ADR 0046), `None` when it is not listed.
    #[cfg(test)]
    pub(crate) fn threads_pane_selected(&self) -> Option<usize> {
        let order = self.threads_pane_ids();
        let cursor = self.thread_cursor();
        let id = cursor.thread()?;
        order.iter().position(|other| other == id)
    }

    /// The first screen row of `lines` drawn, chosen so the highlighted
    /// entry's rows, or its folded file's row, are on screen.
    pub(crate) fn threads_pane_scroll(&self, rows: &[PaneRow], lines: &[PaneLine]) -> usize {
        let body = self.threads_pane_body_rows().max(1);
        let selected = |row: &PaneRow| match row {
            PaneRow::File { selected, .. } => *selected,
            PaneRow::Thread(entry) => entry.selected,
        };
        let end = lines
            .iter()
            .rposition(|line| rows.get(line.row()).is_some_and(selected))
            .map_or(0, |index| index + 1);
        end.saturating_sub(body)
    }

    /// `Space p t`: hide the pane, or show it again without taking the
    /// keys; the tree keeps the sidebar if it is shown.
    pub(crate) fn toggle_threads_pane_shown(&mut self) {
        if !self.sidebar.threads {
            self.show_threads_pane();
            return;
        }
        self.sidebar.hide_threads();
        if self.focus == Focus::ThreadsPane {
            self.focus = Focus::View;
        }
        self.relayout();
    }

    /// Show the pane without taking the keys, as a workspace start does.
    pub(crate) fn show_threads_pane(&mut self) {
        if !self.sidebar.threads {
            self.sidebar.show_threads();
            self.relayout();
        }
    }

    /// Show the pane and give it the keys.
    pub(crate) fn focus_threads_pane(&mut self) {
        self.show_threads_pane();
        self.focus = Focus::ThreadsPane;
    }

    /// `t` in the files pane, or its menu's `threads` (ADR 0066): the
    /// pane in file scope on the highlighted file, with the keys.
    pub(crate) fn threads_on_file(&mut self) {
        self.show_highlight();
        self.sidebar.scope = PaneScope::File;
        self.focus_threads_pane();
    }

    /// Esc in the pane: the keys go back to the text; the pane stays.
    pub(crate) fn leave_threads_pane(&mut self) {
        if self.focus == Focus::ThreadsPane {
            self.focus = Focus::View;
        }
    }

    /// `j` / `k` and the wheel: the next or previous listed thread,
    /// wrapping, a folded file counting once; the text follows, another
    /// file opening in workspace scope, and the keys stay here.
    pub(crate) fn threads_pane_move(&mut self, delta: isize) {
        let order = self.threads_pane_stops();
        if order.is_empty() {
            self.notice(self.empty_pane_notice());
            return;
        }
        if let Some(id) = self.step_in(&order, delta) {
            self.land_in_pane(id);
        }
    }

    /// `s`: list this file, or the whole workspace.
    pub(crate) fn threads_pane_toggle_scope(&mut self) {
        self.sidebar.scope = match self.sidebar.scope {
            PaneScope::File => PaneScope::Workspace,
            PaneScope::Workspace => PaneScope::File,
        };
        self.notice(format!("threads: {}", self.sidebar.scope.word()));
    }

    /// `z` in the pane (ADR 0066): fold the cursor's file to its row, or
    /// unfold it; in file scope there is nothing to fold.
    pub(crate) fn threads_pane_fold(&mut self) {
        if self.sidebar.scope != PaneScope::Workspace {
            return;
        }
        let Some(path) = self
            .thread_cursor()
            .thread()
            .and_then(|id| self.thread(id))
            .map(|thread| thread.path().to_path_buf())
        else {
            return;
        };
        self.threads_pane_toggle_fold(&path);
    }

    /// `Z` in the pane (ADR 0066): fold every listed file, or unfold them
    /// all when any is folded.
    pub(crate) fn threads_pane_fold_all(&mut self) {
        if self.sidebar.scope != PaneScope::Workspace {
            return;
        }
        let listed: HashSet<PathBuf> = self
            .review_entries(false)
            .into_iter()
            .map(|entry| entry.path().to_path_buf())
            .collect();
        if listed.iter().any(|path| self.sidebar.folded.contains(path)) {
            self.sidebar.folded.clear();
        } else {
            self.sidebar.folded = listed;
        }
    }

    fn threads_pane_toggle_fold(&mut self, path: &Path) {
        if !self.sidebar.folded.remove(path) {
            self.sidebar.folded.insert(path.to_path_buf());
        }
    }

    /// A click on the pane's rule row or header: the keys come here.
    pub(crate) fn threads_pane_focus(&mut self) {
        self.focus_pane(Focus::ThreadsPane);
    }

    /// A click on body row `row` (counted from the first drawn row): on
    /// a file row the file folds or unfolds; on either row of a thread
    /// the cursor goes to it; the keys stay with this pane. A click past
    /// the rows acts as one on the header.
    pub(crate) fn threads_pane_click(&mut self, row: usize) {
        match self.threads_pane_at(row) {
            Some(PaneRow::File { path, .. }) => {
                self.threads_pane_toggle_fold(&path);
                self.threads_pane_focus();
            }
            Some(PaneRow::Thread(entry)) => self.land_in_pane(entry.id),
            None => self.threads_pane_focus(),
        }
    }

    /// A right-click on body row `row` (ADR 0066): on a file row the
    /// cursor goes to the file's first thread; on a thread, to it.
    pub(crate) fn threads_pane_point(&mut self, row: usize) -> Option<PanePoint> {
        match self.threads_pane_at(row)? {
            PaneRow::File { path, .. } => {
                let first = self
                    .review_entries(false)
                    .into_iter()
                    .find(|entry| entry.path() == path)
                    .map(|entry| entry.id().clone())?;
                self.land_in_pane(first);
                Some(PanePoint::File(path))
            }
            PaneRow::Thread(entry) => {
                self.land_in_pane(entry.id);
                Some(PanePoint::Thread)
            }
        }
    }

    /// The entry drawn on body row `row`.
    fn threads_pane_at(&self, row: usize) -> Option<PaneRow> {
        let rows = self.threads_pane_rows();
        let lines = pane_lines(&rows);
        let line = lines.get(self.threads_pane_scroll(&rows, &lines) + row)?;
        rows.get(line.row()).cloned()
    }

    /// Enter: open the file with the cursor's thread expanded and the
    /// keys going to the text (ADR 0049).
    pub(crate) fn threads_pane_open(&mut self) {
        let Some(id) = self.thread_cursor().thread().cloned() else {
            return;
        };
        if self.land_on_thread(id.clone()) {
            let newest = self.newest_message(&id);
            self.goto_message(id, newest);
            self.focus = Focus::View;
        }
    }

    /// The pane's rule was dragged to screen row `row`.
    pub(crate) fn drag_threads_pane_to(&mut self, row: usize) {
        self.sidebar.split = Some(self.pane_rows().saturating_sub(row));
    }

    /// Land the cursor on `id` from the pane: the file opens if it is
    /// elsewhere and the keys stay with the pane.
    fn land_in_pane(&mut self, id: ThreadId) {
        if self.land_on_thread(id) && self.sidebar.threads {
            self.focus = Focus::ThreadsPane;
        }
    }

    fn empty_pane_notice(&self) -> &'static str {
        match self.sidebar.scope {
            PaneScope::File => "no threads in this file",
            PaneScope::Workspace => "no threads in the workspace",
        }
    }

    /// The mark of `id` on the current document, if it is there.
    pub(crate) fn mark_of(&self, id: &ThreadId) -> Option<&Mark> {
        self.marks().iter().find(|mark| mark.id() == id)
    }
}

#[cfg(test)]
mod tests;
