// @okf-doc: /decisions/0050-mouse-menus-and-gestures.md
//! Context and pane-settings menus (ADR 0050, ADR 0068): the actions
//! that apply under the pointer or to the Files pane, each showing the
//! key the binding table gives it, and the grids that the drawn menus
//! and the mouse share so a click lands on the drawn entry.
//!
//! A [`Menu`] is one more [`Popup`]. Its entries act on the cursor the
//! right-click placed, so they are ordinary [`Action`]s run through
//! [`App::act`]; the menu needs no target of its own.

use std::path::Path;

use super::bindings::{self, Action, Chord, Keys, Match, Where};
use crate::app::threads::pane::{PanePoint, PaneScope};
use crate::app::threads::words::Words;
use crate::app::view::{Effect, Mode};
use crate::app::{App, Focus, Popup};
use fathomable_core::layout::display_width;
use fathomable_core::tree::Tree;

/// One row of a context menu.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    /// The key as the table spells it, shown beside the label.
    key: String,
    /// The chords that key is, matched against what is typed.
    keys: Keys,
    label: String,
    /// What the entry runs. The key shown may belong to a sibling
    /// action: `dd` arms a delete, the entry deletes at once.
    action: Action,
    /// `Some` for a persistent toggle row, active or inactive.
    checked: Option<bool>,
}

impl Entry {
    #[must_use]
    pub(crate) fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub(crate) fn action(&self) -> Action {
        self.action
    }

    #[must_use]
    pub(crate) fn checked(&self) -> Option<bool> {
        self.checked
    }
}

/// A context menu: what it acts on, its entries, and the cell it opened
/// at.
#[derive(Debug, Clone)]
pub(crate) struct Menu {
    title: String,
    place: Where,
    entries: Vec<Entry>,
    column: usize,
    row: usize,
}

impl Menu {
    fn new(title: impl Into<String>, place: Where, column: usize, row: usize) -> Self {
        Self {
            title: title.into(),
            place,
            entries: Vec::new(),
            column,
            row,
        }
    }

    /// A pane-title menu whose top border sits immediately below its header.
    fn below_header(title: impl Into<String>, place: Where, row: usize) -> Self {
        Self::new(title, place, 0, row.saturating_add(1))
    }

    /// Add an entry showing `shown`'s key and running `run`; an action
    /// the table does not bind on this place adds nothing, so no entry
    /// is ever keyless.
    fn push(&mut self, shown: Action, run: Action, label: impl Into<String>) {
        self.push_entry(shown, run, label, None);
    }

    /// Add a persistent toggle entry carrying its current checked state.
    fn push_toggle(&mut self, shown: Action, run: Action, label: impl Into<String>, active: bool) {
        self.push_entry(shown, run, label, Some(active));
    }

    fn push_entry(
        &mut self,
        shown: Action,
        run: Action,
        label: impl Into<String>,
        checked: Option<bool>,
    ) {
        if let Some(keys) = bindings::first_keys(self.place, shown) {
            self.entries.push(Entry {
                key: bindings::menu_spell(keys),
                keys,
                label: label.into(),
                action: run,
                checked,
            });
        }
    }

    /// The row in the pill colour naming what the menu acts on.
    #[must_use]
    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub(crate) fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// What `typed` means here: an entry's whole key fires it, the start
    /// of one waits, anything else is a miss that closes the menu.
    #[must_use]
    pub(crate) fn typed(&self, typed: &[Chord]) -> Match {
        if let Some(entry) = self.entries.iter().find(|entry| entry.keys == typed) {
            return Match::Exact(entry.action);
        }
        if self
            .entries
            .iter()
            .any(|entry| entry.keys.len() > typed.len() && entry.keys.starts_with(typed))
        {
            return Match::Prefix;
        }
        Match::Miss
    }

    /// Where the menu sits on a `width` by `height` screen: its top-left
    /// corner at the pointer, shifted left or up to stay inside.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn grid(&self, width: usize, height: usize) -> Grid {
        self.grid_in(width, 0, height)
    }

    /// Place the menu inside a vertical area beginning at `top`.
    #[must_use]
    pub(crate) fn grid_in(&self, width: usize, top: usize, height: usize) -> Grid {
        let (key_width, label_width) = measure(
            self.entries
                .iter()
                .map(|entry| (entry.key.as_str(), entry.label.as_str())),
        );
        let check_width = usize::from(self.entries.iter().any(|entry| entry.checked.is_some())) * 2;
        let action_width = check_width + label_width + 1 + key_width;
        let box_width = (action_width + 2)
            .max(display_width(&self.title) + 4)
            .min(width);
        let box_height = (self.entries.len() + 2).min(height);
        Grid {
            x: self.column.min(width.saturating_sub(box_width)),
            y: self
                .row
                .max(top)
                .min(top + height.saturating_sub(box_height)),
            width: box_width,
            height: box_height,
            rows: self.entries.len(),
            columns: 1,
            key_width,
            label_width,
            count: self.entries.len(),
        }
    }
}

/// The widest key and the widest label among `entries`.
fn measure<'a>(entries: impl Iterator<Item = (&'a str, &'a str)>) -> (usize, usize) {
    entries.fold((1, 1), |(key, label), (k, l)| {
        (key.max(display_width(k)), label.max(display_width(l)))
    })
}

/// Where a key menu's rows sit inside a rounded titled border. The drawing
/// lays the entries out by it and the mouse reads it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Grid {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) rows: usize,
    pub(crate) columns: usize,
    pub(crate) key_width: usize,
    pub(crate) label_width: usize,
    pub(crate) count: usize,
}

impl Grid {
    /// The cells one column of entries takes.
    #[must_use]
    pub(crate) fn column_width(key_width: usize, label_width: usize) -> usize {
        key_width + 2 + label_width + 3
    }

    /// The which-key menu (ADR 0045) at the bottom right of the area at
    /// `(x, y)` of `pane_width` by `pane_height`: as many columns as
    /// keep the box to eight rows.
    #[must_use]
    pub(crate) fn bottom(
        entries: &[(String, String)],
        title: &str,
        x: usize,
        y: usize,
        pane_width: usize,
        pane_height: usize,
    ) -> Self {
        let (key_width, label_width) =
            measure(entries.iter().map(|(k, l)| (k.as_str(), l.as_str())));
        let max_rows = pane_height.saturating_sub(2).clamp(1, 8);
        let columns = entries.len().div_ceil(max_rows).max(1);
        let rows = entries.len().div_ceil(columns).max(1);
        let width = (columns * Self::column_width(key_width, label_width) + 2)
            .max(display_width(title) + 4)
            .min(pane_width);
        let height = (rows + 2).min(pane_height);
        Self {
            x: x + pane_width.saturating_sub(width),
            y: (y + pane_height).saturating_sub(height),
            width,
            height,
            rows,
            columns,
            key_width,
            label_width,
            count: entries.len(),
        }
    }

    /// A centred table in a rounded titled border. Rows that do not fit
    /// flow into further columns; the box sits a third of the way down.
    #[must_use]
    pub(crate) fn centred(
        rows: &[(String, String)],
        title: &str,
        x: usize,
        y: usize,
        area_width: usize,
        area_height: usize,
    ) -> Self {
        let (key_width, label_width) = measure(rows.iter().map(|(k, l)| (k.as_str(), l.as_str())));
        let per_column = area_height.saturating_sub(3).max(1);
        let columns = rows.len().div_ceil(per_column).max(1);
        let per_column = rows.len().div_ceil(columns).max(1);
        let body = columns * Self::column_width(key_width, label_width);
        let width = (body.max(display_width(title) + 2) + 2).min(area_width);
        let height = (per_column.min(rows.len()) + 2).min(area_height);
        Self {
            x: x + (area_width - width) / 2,
            y: y + (area_height - height) / 3,
            width,
            height,
            rows: per_column,
            columns,
            key_width,
            label_width,
            count: rows.len(),
        }
    }

    /// Whether the cell is inside the box, border included.
    #[must_use]
    pub(crate) fn contains(&self, column: usize, row: usize) -> bool {
        column >= self.x
            && column < self.x + self.width
            && row >= self.y
            && row < self.y + self.height
    }

    /// The entry drawn at the cell, if any.
    #[must_use]
    pub(crate) fn entry_at(&self, column: usize, row: usize) -> Option<usize> {
        if !self.contains(column, row)
            || row == self.y
            || row + 1 == self.y + self.height
            || column == self.x
            || column + 1 == self.x + self.width
        {
            return None;
        }
        let r = row - self.y - 1;
        let c = if self.columns == 1 {
            0
        } else {
            (column.checked_sub(self.x + 1)?) / Self::column_width(self.key_width, self.label_width)
        };
        if r >= self.rows || c >= self.columns {
            return None;
        }
        let index = c * self.rows + r;
        (index < self.count).then_some(index)
    }
}

impl App {
    /// The open context menu, if one is.
    #[must_use]
    pub(crate) fn menu(&self) -> Option<&Menu> {
        match self.popup() {
            Some(Popup::Menu(menu)) => Some(menu),
            _ => None,
        }
    }

    fn open_menu(&mut self, menu: Menu) {
        self.take_prefix();
        self.cancel_delete();
        self.park_draft();
        self.popup = Some(Popup::Menu(menu));
    }

    /// A key while the menu is open: an entry's key runs it, the start
    /// of one waits, anything else closes the menu and is swallowed.
    pub(super) fn menu_key(&mut self, chord: Chord) -> Effect {
        let mut typed = self.take_prefix();
        typed.push(chord);
        let Some(menu) = self.menu() else {
            return Effect::None;
        };
        match menu.typed(&typed) {
            Match::Exact(action) => {
                self.close_popup();
                self.act(action)
            }
            Match::Prefix => {
                self.set_prefix(typed);
                Effect::None
            }
            Match::Miss => {
                self.close_popup();
                Effect::None
            }
        }
    }

    /// A click on entry `index` of the open menu.
    pub(super) fn menu_click(&mut self, index: usize) -> Effect {
        let Some(action) = self
            .menu()
            .and_then(|menu| menu.entries.get(index))
            .map(Entry::action)
        else {
            return Effect::None;
        };
        self.close_popup();
        self.act(action)
    }

    /// A right-click in the text at screen `(column, row)`, `screen_row`
    /// rows into the text and `col` cells into it: a selection the
    /// pointer is in stays, else the cursor goes there; then the menu
    /// for what is under the cursor.
    pub(super) fn open_view_menu(
        &mut self,
        screen_row: usize,
        col: usize,
        column: usize,
        row: usize,
    ) {
        self.focus_pane(Focus::View);
        let view = self.view();
        let inside = view.mode() == Mode::Select
            && view
                .selection()
                .is_some_and(|selection| selection.contains(view.scroll() + screen_row, col));
        if !inside {
            self.view_mut().click(screen_row, col);
        }
        let menu = self.view_menu(column, row);
        self.open_menu(menu);
    }

    fn view_menu(&self, column: usize, row: usize) -> Menu {
        let place = Where::View;
        let view = self.view();
        if view.mode() == Mode::Select && view.selection().is_some() {
            let mut menu = Menu::new("selection", place, column, row);
            menu.push(Action::Comment, Action::Comment, "comment on selection");
            menu.push(
                Action::NewThread,
                Action::NewThread,
                "new thread on selection",
            );
            menu.push(Action::Yank, Action::Yank, "copy selection");
            menu.push(Action::Escape, Action::Escape, "clear selection");
            return menu;
        }
        let threads = self.threads_at_cursor();
        let title = if threads.is_empty() {
            view.cursor_source_line()
                .map_or_else(|| "line".to_owned(), |line| format!("line {line}"))
        } else {
            "thread".to_owned()
        };
        let mut menu = Menu::new(title, place, column, row);
        if let Some(id) = threads.first() {
            let on_thread_row = self.cursor_on_thread_row(id);
            let on_expanded = self.expanded_row_message(view.cursor().row).is_some();
            menu.push(
                Action::Fold,
                Action::Fold,
                if on_expanded {
                    "fold thread"
                } else {
                    "expand thread"
                },
            );
            if on_thread_row {
                menu.push(Action::Comment, Action::Comment, "reply");
            } else {
                menu.push(Action::Reply, Action::Reply, "reply");
            }
            let resolved = self
                .thread(id)
                .is_some_and(|thread| Words::of(None, thread).is_resolved());
            if !resolved {
                let enabled = self
                    .thread(id)
                    .is_some_and(|thread| thread.auto_resolve().is_enabled());
                menu.push(
                    Action::ToggleAutoResolve,
                    Action::ToggleAutoResolve,
                    if enabled {
                        "disable auto-resolve"
                    } else {
                        "enable auto-resolve"
                    },
                );
            }
            menu.push(
                Action::ToggleResolved,
                Action::ToggleResolved,
                if resolved {
                    "reopen thread"
                } else {
                    "resolve thread"
                },
            );
            if resolved {
                menu.push(
                    Action::ArchiveThread,
                    Action::ArchiveThread,
                    "archive thread",
                );
            }
            if self.thread_message_editable() {
                menu.push(Action::EditMessage, Action::EditMessage, "edit message");
            }
            menu.push(Action::Delete, Action::DeleteThread, "delete thread");
        }
        if self.reference_here() {
            menu.push(Action::GotoFile, Action::GotoFile, "open linked file/URL");
        }
        menu.push(Action::Comment, Action::Comment, "comment on line");
        menu.push(Action::ExtendLine, Action::ExtendLine, "select line");
        menu.push(Action::Yank, Action::Yank, "copy line");
        menu
    }

    /// A left-click on the Files title opens display settings below the header.
    pub(super) fn open_files_menu(&mut self, row: usize) {
        let mut menu = Menu::below_header("Files", Where::Tree, row);
        for action in [
            Action::FilesChanged,
            Action::FilesUntracked,
            Action::FilesIgnored,
        ] {
            menu.push_toggle(
                action,
                action,
                Self::files_setting_label(action),
                self.files_setting_checked(action),
            );
        }
        self.open_menu(menu);
    }

    /// A left-click on the Threads title opens view settings below the header.
    pub(super) fn open_threads_menu(&mut self, row: usize) {
        let mut menu = Menu::below_header("Threads", Where::ThreadsPane, row);
        menu.push_toggle(
            Action::PaneScope,
            Action::PaneScope,
            "only current file",
            self.sidebar_scope() == PaneScope::File,
        );
        menu.push_toggle(
            Action::ReviewResolved,
            Action::ReviewResolved,
            "show resolved",
            self.review().resolved,
        );
        self.open_menu(menu);
    }

    /// A right-click on tree row `tree_row` at screen `(column, row)`:
    /// the highlight moves there, showing the file as the wheel does,
    /// and the menu offers what the row can do.
    pub(super) fn open_tree_menu(&mut self, tree_row: usize, column: usize, row: usize) {
        self.tree_point(tree_row);
        let Some(current) = self.tree().and_then(Tree::current) else {
            return;
        };
        let is_dir = current.is_dir();
        let mut menu = Menu::new(current.name().to_owned(), Where::Tree, column, row);
        if is_dir {
            menu.push(
                Action::Confirm,
                Action::Confirm,
                if current.expanded() {
                    "collapse"
                } else {
                    "expand"
                },
            );
        } else {
            menu.push(Action::Confirm, Action::Confirm, "open");
            menu.push(Action::FileComment, Action::FileComment, "file comment");
        }
        menu.push(Action::CopyPath, Action::CopyPath, "copy path");
        menu.push(Action::CopyFullPath, Action::CopyFullPath, "copy full path");
        self.open_menu(menu);
    }

    /// A right-click on a threads pane row at screen `(column, row)`:
    /// on a thread the cursor moves there and the thread's menu opens;
    /// on a file row the cursor goes to its first thread and the file's
    /// menu opens (ADR 0066).
    pub(super) fn open_threads_pane_menu(&mut self, entry_row: usize, column: usize, row: usize) {
        match self.threads_pane_point(entry_row) {
            Some(PanePoint::File(path)) => {
                let folded = self.threads_pane_is_folded(&path);
                let menu = self.file_menu(Where::ThreadsPane, &path, folded, column, row);
                self.open_menu(menu);
            }
            Some(PanePoint::Thread) => {
                let menu = self.thread_menu(Where::ThreadsPane, column, row);
                self.open_menu(menu);
            }
            None => {}
        }
    }

    /// A right-click on a review list row at screen `(column, row)`: a
    /// file row's menu, or the thread's.
    pub(super) fn open_review_menu(&mut self, list_row: usize, column: usize, row: usize) {
        if let Some(path) = self.review_point(list_row) {
            let folded = self.review_list().is_folded(&path);
            let menu = self.file_menu(Where::Review, &path, folded, column, row);
            self.open_menu(menu);
            return;
        }
        self.review_click(list_row);
        if self.thread_cursor().thread().is_some() {
            let menu = self.thread_menu(Where::Review, column, row);
            self.open_menu(menu);
        }
    }

    /// Whether `z` on a thread row of `place` folds the thread's file
    /// (ADR 0066): only in the pane in workspace scope; in the list `z`
    /// folds the thread (ADR 0076).
    fn folds_files(&self, place: Where) -> bool {
        match place {
            Where::ThreadsPane => self.sidebar_scope() == PaneScope::Workspace,
            _ => false,
        }
    }

    /// The menu for a file row (ADR 0066): fold or unfold it, in the
    /// pane fold or unfold every file (the list's `Z` folds threads, ADR
    /// 0076), then open the file. The review list also carries its resolved
    /// toggle; the pane keeps that setting in its title menu.
    fn file_menu(
        &self,
        place: Where,
        path: &Path,
        folded: bool,
        column: usize,
        row: usize,
    ) -> Menu {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let mut menu = Menu::new(name, place, column, row);
        menu.push(
            Action::Fold,
            Action::Fold,
            if folded { "unfold" } else { "fold" },
        );
        if place == Where::ThreadsPane {
            let any_folded = self
                .review_entries(false)
                .iter()
                .any(|entry| self.threads_pane_is_folded(entry.path()));
            menu.push(
                Action::FoldAll,
                Action::FoldAll,
                if any_folded { "unfold all" } else { "fold all" },
            );
        }
        menu.push(Action::Confirm, Action::Confirm, "open file");
        if place == Where::Review {
            menu.push(
                Action::ReviewResolved,
                Action::ReviewResolved,
                if self.review().resolved {
                    "hide resolved"
                } else {
                    "show resolved"
                },
            );
        }
        menu
    }

    /// The menu for the thread cursor's thread on a thread surface; in
    /// the list it folds and expands the thread as the text's does (ADR
    /// 0076).
    fn thread_menu(&self, place: Where, column: usize, row: usize) -> Menu {
        let mut menu = Menu::new("thread", place, column, row);
        if place == Where::Review {
            let folded = self
                .thread_cursor()
                .thread()
                .is_some_and(|id| self.review_list().is_thread_folded(id));
            menu.push(
                Action::Fold,
                Action::Fold,
                if folded {
                    "expand thread"
                } else {
                    "fold thread"
                },
            );
        }
        menu.push(Action::Confirm, Action::Confirm, "go to");
        let archived = self
            .thread_cursor()
            .thread()
            .and_then(|id| self.thread(id))
            .is_some_and(fathomable_core::annotations::Thread::is_archived);
        if archived {
            menu.push(
                Action::RestoreThread,
                Action::RestoreThread,
                "restore thread",
            );
            return menu;
        }
        menu.push(Action::Reply, Action::Reply, "reply");
        let resolved = self
            .thread_cursor()
            .thread()
            .and_then(|id| self.thread(id))
            .is_some_and(|thread| Words::of(None, thread).is_resolved());
        if !resolved {
            let enabled = self
                .thread_cursor()
                .thread()
                .and_then(|id| self.thread(id))
                .is_some_and(|thread| thread.auto_resolve().is_enabled());
            menu.push(
                Action::ToggleAutoResolve,
                Action::ToggleAutoResolve,
                if enabled {
                    "disable auto-resolve"
                } else {
                    "enable auto-resolve"
                },
            );
        }
        menu.push(
            Action::ToggleResolved,
            Action::ToggleResolved,
            if resolved {
                "reopen thread"
            } else {
                "resolve thread"
            },
        );
        if resolved {
            menu.push(
                Action::ArchiveThread,
                Action::ArchiveThread,
                "archive thread",
            );
        }
        if place == Where::Review {
            if self.thread_message_editable() {
                menu.push(Action::EditMessage, Action::EditMessage, "edit message");
            }
        } else {
            menu.push(
                Action::EditNewestOwn,
                Action::EditNewestOwn,
                "edit your newest message",
            );
        }
        menu.push(Action::Delete, Action::DeleteThread, "delete thread");
        if self.folds_files(place) {
            menu.push(Action::Fold, Action::Fold, "fold file");
        }
        menu
    }
}

#[cfg(test)]
mod tests;
