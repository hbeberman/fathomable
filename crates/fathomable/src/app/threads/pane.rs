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

use crate::app::threads::cursor::ThreadLanding;
use crate::app::threads::summary::ThreadSummary;
use crate::app::threads::words::Words;
use crate::app::threads::{Mark, ThreadState};
use crate::app::{App, Focus};

/// Rows the pane needs before its entries: the rule and the header.
const CHROME_ROWS: usize = 2;
/// The fewest rows the focused pane needs: rule, header, entry, and footer.
const MIN_ROWS: usize = 4;
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

    /// The header's narrow fallback.
    #[must_use]
    pub(crate) fn short_word(self) -> &'static str {
        match self {
            Self::File => "f",
            Self::Workspace => "w",
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
    /// The thread is on the current document.
    current: bool,
    /// The thread cursor's thread.
    selected: bool,
    /// The branch of the worktree showing it (ADR 0070), or the commit
    /// a past thread was resolved at (ADR 0072).
    summary_facts: ThreadSummary,
}

impl PaneEntry {
    /// After the author: the branch when another worktree shows the
    /// thread (ADR 0070), the commit when it is resolved at an earlier
    /// one of this branch (ADR 0072).
    pub(crate) fn summary_facts(&self) -> &ThreadSummary {
        &self.summary_facts
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

    #[cfg(test)]
    pub(crate) fn author(&self) -> &str {
        self.summary_facts.author()
    }

    #[cfg(test)]
    pub(crate) fn author_name(&self) -> &str {
        self.summary_facts.author()
    }

    #[cfg(test)]
    pub(crate) fn summary(&self) -> &str {
        self.summary_facts.preview()
    }

    #[cfg(test)]
    pub(crate) fn replies(&self) -> usize {
        self.summary_facts.replies()
    }

    #[cfg(test)]
    pub(crate) fn updated(&self) -> u64 {
        self.summary_facts.modified()
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
    #[cfg(test)]
    pub(crate) fn place(&self) -> String {
        self.summary_facts.location().to_owned()
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

    /// Whether the file group is effectively collapsed in the Thread list.
    pub(crate) fn threads_pane_file_is_collapsed(&self, path: &Path) -> bool {
        self.threads_pane_is_folded(path) && !self.pane_file_peeked(path)
    }

    /// The threads the pane lists, in its order, resolved ones only when
    /// the review shows them: this file's by line, or the workspace's by
    /// file and line (ADR 0066).
    pub(super) fn threads_pane_ids(&self) -> Vec<ThreadId> {
        self.normal_review_entries(self.sidebar.scope == PaneScope::File)
            .into_iter()
            .map(|entry| entry.id().clone())
            .collect()
    }

    /// Open threads admitted by the pane's current scope and lifecycle filter.
    pub(super) fn threads_pane_open_threads(&self) -> Vec<ThreadId> {
        self.normal_review_entries(self.sidebar.scope == PaneScope::File)
            .into_iter()
            .filter(|entry| matches!(entry.kind(), ThreadState::Active | ThreadState::Proposed))
            .map(|entry| entry.id().clone())
            .collect()
    }

    /// The threads `j` / `k` stop on: every listed thread,
    /// a folded file counting once through its first thread (ADR 0066).
    fn threads_pane_stops(&self) -> Vec<ThreadId> {
        let mut last_path: Option<PathBuf> = None;
        let mut out = Vec::new();
        for entry in self.normal_review_entries(self.sidebar.scope == PaneScope::File) {
            let folded = self.sidebar.scope == PaneScope::Workspace
                && self.threads_pane_file_is_collapsed(entry.path());
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
        let cursor = self.threads_pane_thread_cursor().thread().cloned();
        let entries = self.normal_review_entries(!grouped);
        let mut out = Vec::new();
        let mut index = 0;
        while index < entries.len() {
            let path = entries[index].path().to_path_buf();
            let group_end = entries[index..]
                .iter()
                .position(|entry| entry.path() != path)
                .map_or(entries.len(), |len| index + len);
            let folded = grouped && self.threads_pane_file_is_collapsed(&path);
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
                    let summary_facts = entry.summary().clone();
                    out.push(PaneRow::Thread(PaneEntry {
                        id: entry.id().clone(),
                        path: path.clone(),
                        range: entry.range(),
                        words: entry.words(),
                        current: path == self.current_path(),
                        selected: cursor.as_ref() == Some(entry.id()),
                        summary_facts,
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

    /// Rows the pane's entries have after its header and contextual key bar.
    pub(crate) fn threads_pane_body_rows(&self) -> usize {
        self.threads_pane_height()
            .saturating_sub(CHROME_ROWS)
            .saturating_sub(usize::from(self.pane_has_navigation(Focus::ThreadsPane)))
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
    pub(crate) fn threads_pane_selected(&self) -> Option<usize> {
        let order = self.threads_pane_ids();
        let cursor = self.threads_pane_thread_cursor();
        let id = cursor.thread()?;
        order.iter().position(|other| other == id)
    }

    /// The cursor thread while it is represented by the pane's current list.
    pub(crate) fn threads_pane_cursor_thread(
        &self,
    ) -> Option<&fathomable_core::annotations::Thread> {
        let cursor = self.threads_pane_thread_cursor();
        let id = cursor.thread()?;
        let file_only = self.sidebar.scope == PaneScope::File;
        self.normal_review_entries(file_only)
            .iter()
            .any(|entry| entry.id() == id)
            .then(|| self.thread(id))
            .flatten()
    }

    /// Re-select after a store change removes the pane's cursor entry.
    pub(crate) fn threads_pane_reselect(&mut self, place: Option<usize>) {
        let ids = self.threads_pane_ids();
        let cursor = self.threads_pane_cursor.clone();
        let index = cursor
            .thread()
            .and_then(|id| ids.iter().position(|other| other == id))
            .or_else(|| place.map(|place| place.min(ids.len().saturating_sub(1))));
        if let Some(index) = index.filter(|_| !ids.is_empty()) {
            let id = ids[index].clone();
            self.set_threads_pane_cursor(&id, self.newest_message(&id));
        } else {
            self.threads_pane_cursor = super::cursor::ThreadCursor::default();
        }
    }

    /// Make the sidebar's actionable cursor belong to its current scope.
    ///
    /// A file switch can leave the retained sidebar history on another
    /// file. Prefer the current main surface's admitted seat, then the
    /// first displayed thread; an empty pane has no action target.
    pub(crate) fn reconcile_threads_pane_cursor(&mut self) {
        let ids = self.threads_pane_ids();
        if let Some(id) = self.threads_pane_cursor.thread().cloned()
            && ids.contains(&id)
        {
            let message = self
                .threads_pane_cursor
                .message()
                .min(self.newest_message(&id));
            self.set_threads_pane_cursor(&id, message);
            return;
        }
        let candidate = if self.review_list().is_open() {
            self.review_thread_cursor()
        } else {
            self.file_thread_cursor()
        };
        let id = candidate
            .thread()
            .filter(|id| ids.contains(id))
            .cloned()
            .or_else(|| ids.first().cloned());
        if let Some(id) = id {
            let message = if candidate.thread() == Some(&id) {
                candidate.message()
            } else {
                self.newest_message(&id)
            };
            self.set_threads_pane_cursor(&id, message);
        } else {
            self.threads_pane_cursor = super::cursor::ThreadCursor::default();
        }
    }

    /// The first body row drawn from the pane's independent viewport.
    pub(crate) fn threads_pane_scroll(&self, _rows: &[PaneRow], lines: &[PaneLine]) -> usize {
        let body = self.threads_pane_body_rows().max(1);
        self.threads_pane_scroll
            .min(lines.len().saturating_sub(body))
    }

    /// Move only the Thread-list viewport, leaving its cursor and preview.
    pub(crate) fn scroll_threads_pane(&mut self, delta: isize) {
        let lines = pane_lines(&self.threads_pane_rows());
        let max = lines
            .len()
            .saturating_sub(self.threads_pane_body_rows().max(1));
        self.threads_pane_scroll = self
            .threads_pane_scroll
            .saturating_add_signed(delta)
            .min(max);
    }

    /// Reveal the selected entry after deliberate keyboard or click selection.
    pub(super) fn reveal_threads_pane_selection(&mut self) {
        let rows = self.threads_pane_rows();
        let lines = pane_lines(&rows);
        let body = self.threads_pane_body_rows().max(1);
        let selected = |row: &PaneRow| match row {
            PaneRow::File { selected, .. } => *selected,
            PaneRow::Thread(entry) => entry.selected,
        };
        let Some(end) = lines
            .iter()
            .rposition(|line| rows.get(line.row()).is_some_and(selected))
            .map(|index| index + 1)
        else {
            self.threads_pane_scroll = self
                .threads_pane_scroll
                .min(lines.len().saturating_sub(body));
            return;
        };
        let start = lines
            .iter()
            .position(|line| rows.get(line.row()).is_some_and(selected))
            .unwrap_or(end - 1);
        if start < self.threads_pane_scroll {
            self.threads_pane_scroll = start;
        } else if end > self.threads_pane_scroll + body {
            self.threads_pane_scroll = end.saturating_sub(body);
        }
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
            self.focus = self.displayed_main_focus();
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
        if self.panes_fit() {
            self.reconcile_threads_pane_cursor();
            self.focus = Focus::ThreadsPane;
            self.reveal_threads_pane_selection();
        }
    }

    /// Esc in the pane: the keys go back to the text; the pane stays.
    pub(crate) fn leave_threads_pane(&mut self) {
        if self.focus == Focus::ThreadsPane {
            self.focus = self.displayed_main_focus();
        }
    }

    /// `j` / `k`: the next or previous listed thread,
    /// wrapping, a folded file counting once; the text follows, another
    /// file opening in workspace scope, and the keys stay here.
    pub(crate) fn threads_pane_move(&mut self, delta: isize) {
        let mut order = self.threads_pane_stops();
        if order.is_empty() {
            self.notice(self.empty_pane_notice());
            return;
        }
        if self.sidebar.scope == PaneScope::Workspace
            && let Some(selected) = self.threads_pane_thread_cursor().thread().cloned()
            && !order.contains(&selected)
            && let Some(path) = self
                .thread(&selected)
                .map(|thread| self.thread_path(thread).to_path_buf())
            && let Some(representative) = order.iter_mut().find(|id| {
                self.thread(id)
                    .is_some_and(|thread| self.thread_path(thread) == path)
            })
        {
            *representative = selected;
        }
        if let Some(id) = self.step_in(&order, delta) {
            self.land_in_pane(&id);
            self.reveal_threads_pane_selection();
        }
    }

    /// `s`: list this file, or the whole workspace.
    pub(crate) fn threads_pane_toggle_scope(&mut self) {
        self.dismiss_navigation_peek();
        self.sidebar.scope = match self.sidebar.scope {
            PaneScope::File => PaneScope::Workspace,
            PaneScope::Workspace => PaneScope::File,
        };
        self.reconcile_threads_pane_cursor();
        self.reveal_threads_pane_selection();
        self.notice(format!("threads: {}", self.sidebar.scope.word()));
    }

    /// `z` in the pane (ADR 0066): fold the cursor's file to its row, or
    /// unfold it; in file scope there is nothing to fold.
    pub(crate) fn threads_pane_fold(&mut self) {
        if self.sidebar.scope != PaneScope::Workspace {
            return;
        }
        let Some(path) = self
            .threads_pane_thread_cursor()
            .thread()
            .and_then(|id| self.thread(id))
            .map(|thread| self.thread_path(thread).to_path_buf())
        else {
            return;
        };
        self.threads_pane_toggle_fold(&path);
        self.reveal_threads_pane_selection();
    }

    /// `Z` in the pane (ADR 0066): fold every listed file, or unfold them
    /// all when any is folded.
    pub(crate) fn threads_pane_fold_all(&mut self) {
        if self.sidebar.scope != PaneScope::Workspace {
            return;
        }
        let listed: HashSet<PathBuf> = self
            .normal_review_entries(false)
            .into_iter()
            .map(|entry| entry.path().to_path_buf())
            .collect();
        let any_folded = listed
            .iter()
            .any(|path| self.threads_pane_file_is_collapsed(path));
        self.release_all_pane_file_peeks();
        if any_folded {
            self.sidebar.folded.clear();
        } else {
            self.sidebar.folded = listed;
        }
        self.reveal_threads_pane_selection();
    }

    fn threads_pane_toggle_fold(&mut self, path: &Path) {
        let visibly_folded = self.threads_pane_file_is_collapsed(path);
        self.release_pane_file_peek(path);
        if visibly_folded {
            self.sidebar.folded.remove(path);
        } else {
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
        let clicked = self.threads_pane_at(row);
        self.threads_pane_focus();
        match clicked {
            Some(PaneRow::File { path, .. }) => {
                self.threads_pane_toggle_fold(&path);
                self.reveal_threads_pane_selection();
            }
            Some(PaneRow::Thread(entry)) => {
                self.land_in_pane(&entry.id);
                self.reveal_threads_pane_selection();
            }
            None => self.threads_pane_focus(),
        }
    }

    /// A right-click on body row `row` (ADR 0066): on a file row the
    /// cursor goes to the file's first thread; on a thread, to it.
    pub(crate) fn threads_pane_point(&mut self, row: usize) -> Option<PanePoint> {
        let point = match self.threads_pane_at(row)? {
            PaneRow::File { path, .. } => {
                let first = self
                    .normal_review_entries(false)
                    .into_iter()
                    .find(|entry| entry.path() == path)
                    .map(|entry| entry.id().clone())?;
                self.land_in_pane(&first);
                Some(PanePoint::File(path))
            }
            PaneRow::Thread(entry) => {
                self.land_in_pane(&entry.id);
                Some(PanePoint::Thread)
            }
        };
        self.focus = Focus::ThreadsPane;
        point
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
        let Some(id) = self.threads_pane_thread_cursor().thread().cloned() else {
            return;
        };
        if self.land_on_thread(id.clone()) == Some(ThreadLanding::Source) {
            let newest = self.newest_message(&id);
            self.goto_message(id, newest);
            self.focus = Focus::View;
        }
    }

    /// The pane's rule was dragged to screen row `row`.
    pub(crate) fn drag_threads_pane_to(&mut self, row: usize) {
        self.sidebar.split = Some(self.pane_rows().saturating_sub(row));
    }

    /// Preview `id` from the pane without changing the displayed main view.
    fn land_in_pane(&mut self, id: &ThreadId) {
        let Some(path) = self
            .thread(id)
            .map(|thread| self.thread_path(thread).to_path_buf())
        else {
            return;
        };
        if self.reject_pane_preview(id, path.clone()) {
            return;
        }
        let keep_file_revealed = self.pane_file_peeked(&path);
        let focus = self.focus;
        self.preview_file(&path);
        if self.current_path() != path {
            return;
        }
        self.dismiss_navigation_peek();
        if self.thread_source_is_displayable(id) {
            self.goto_thread(id);
        } else {
            self.notice("source unavailable; press Enter for thread evidence");
        }
        let newest = self.newest_message(id);
        self.set_threads_pane_cursor(id, newest);
        if self.review_list().is_open() {
            self.land_review_thread(id);
        } else {
            self.set_file_thread_cursor(id.clone(), newest);
        }
        if keep_file_revealed {
            self.peek_pane_file(path.clone());
        }
        self.synchronize_tree_to(&path);
        self.focus = focus;
    }

    /// Keep an excluded sidebar destination out of the current main Threads
    /// membership while still moving the sidebar's own selection.
    fn reject_pane_preview(&mut self, id: &ThreadId, path: PathBuf) -> bool {
        if !self.review_list().is_open() || self.review_admits(id) {
            return false;
        }
        self.release_all_pane_file_peeks();
        self.peek_pane_file(path);
        self.set_threads_pane_cursor(id, self.newest_message(id));
        self.reveal_threads_pane_selection();
        self.notice("thread is outside the current Threads view; press Enter to open");
        self.focus = Focus::ThreadsPane;
        true
    }

    /// Preview a Tab-traversal destination while retaining Thread-list focus.
    pub(super) fn land_thread_step_in_pane(&mut self, id: &ThreadId) {
        let Some(path) = self
            .thread(id)
            .map(|thread| self.thread_path(thread).to_path_buf())
        else {
            return;
        };
        let newest = self.newest_message(id);
        self.set_threads_pane_cursor(id, newest);
        if self.reject_pane_preview(id, path.clone()) {
            return;
        }
        let placed_in_file = if self.review_list().is_open() {
            if self.land_review_thread(id) {
                self.peek_pane_file(path);
            }
            false
        } else {
            self.preview_file(&path);
            if self.thread_source_is_displayable(id) {
                self.land_file_thread_jump(id.clone());
                self.peek_pane_file(path);
                true
            } else {
                self.dismiss_navigation_peek();
                self.peek_pane_file(path);
                self.set_threads_pane_cursor(id, newest);
                self.notice("source unavailable; press Enter for thread evidence");
                false
            }
        };
        self.reveal_threads_pane_selection();
        let current = self.current_path().to_path_buf();
        self.synchronize_tree_to(&current);
        self.focus = Focus::ThreadsPane;
        if placed_in_file {
            self.sync_text_height();
            self.restore_navigation_placement();
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
