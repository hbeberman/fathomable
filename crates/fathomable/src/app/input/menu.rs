// @okf-doc: /decisions/0050-mouse-menus-and-gestures.md
//! The context menu a right-click opens (ADR 0050): the actions that
//! apply under the pointer, each showing the key the binding table
//! gives it, and the grids that the drawn menus and the mouse share so
//! a click lands on the entry that was drawn there.
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

    /// Add an entry showing `shown`'s key and running `run`; an action
    /// the table does not bind on this place adds nothing, so no entry
    /// is ever keyless.
    fn push(&mut self, shown: Action, run: Action, label: impl Into<String>) {
        if let Some(keys) = bindings::first_keys(self.place, shown) {
            self.entries.push(Entry {
                key: bindings::spell(keys),
                keys,
                label: label.into(),
                action: run,
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
    #[must_use]
    pub(crate) fn grid(&self, width: usize, height: usize) -> Grid {
        let (key_width, label_width) = measure(
            self.entries
                .iter()
                .map(|entry| (entry.key.as_str(), entry.label.as_str())),
        );
        let column_width = Grid::column_width(key_width, label_width);
        let box_width = (column_width + 1)
            .max(display_width(&self.title) + 2)
            .min(width);
        let box_height = (self.entries.len() + 1).min(height);
        Grid {
            x: self.column.min(width.saturating_sub(box_width)),
            y: self.row.min(height.saturating_sub(box_height)),
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

/// Where a key menu's rows sit on screen: a title row, then `count`
/// entries down `rows` rows and across `columns` columns, each column
/// `key_width + 2 + label_width + 3` cells wide after one leading cell.
/// The drawing lays the entries out by it and the mouse reads it back.
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

    /// The which-key menu (ADR 0045) along the bottom of the pane at
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
        let width = (columns * Self::column_width(key_width, label_width) + 1)
            .max(display_width(title) + 2)
            .min(pane_width);
        let height = rows + 1;
        Self {
            x,
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

    /// A centred table (`Space ?`, `:status`) in the area at `(x, y)`
    /// of `area_width` by `area_height`: rows that do not fit under the
    /// title flow into further columns; the box sits a third of the way
    /// down.
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
        let per_column = area_height.saturating_sub(2).max(1);
        let columns = rows.len().div_ceil(per_column).max(1);
        let per_column = rows.len().div_ceil(columns).max(1);
        let body = columns * Self::column_width(key_width, label_width) - 2;
        let width = (body.max(display_width(title)) + 2).min(area_width);
        let height = (per_column.min(rows.len()) + 1).min(area_height.saturating_sub(1));
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

    /// Whether the cell is inside the box, title row included.
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
        if !self.contains(column, row) || row == self.y {
            return None;
        }
        let r = row - self.y - 1;
        let c = (column.checked_sub(self.x + 1)?)
            / Self::column_width(self.key_width, self.label_width);
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
            menu.push(Action::Reply, Action::Reply, "reply");
            let resolved = self
                .thread(id)
                .is_some_and(|thread| Words::of(None, thread).is_resolved());
            menu.push(
                Action::ToggleResolved,
                Action::ToggleResolved,
                if resolved {
                    "reopen thread"
                } else {
                    "resolve thread"
                },
            );
            if self.thread_message_editable() {
                menu.push(Action::EditMessage, Action::EditMessage, "edit message");
            }
            menu.push(Action::Delete, Action::DeleteThread, "delete thread");
        }
        if view.link_at_cursor().is_some() {
            menu.push(Action::CopyLink, Action::CopyLink, "copy link");
            menu.push(Action::OpenLink, Action::OpenLink, "open link");
        }
        if self.file_here() {
            menu.push(Action::GotoFile, Action::GotoFile, "open in viewer");
        }
        if threads.is_empty() {
            menu.push(Action::Comment, Action::Comment, "comment on line");
        } else {
            menu.push(Action::NewThread, Action::NewThread, "new thread on line");
        }
        menu.push(Action::ExtendLine, Action::ExtendLine, "select line");
        menu.push(Action::Yank, Action::Yank, "copy line");
        menu
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
        let has_threads = !is_dir
            && self
                .file_circles()
                .iter()
                .any(|(path, _)| path == current.path());
        let mut menu = Menu::new(current.name().to_owned(), Where::Tree, column, row);
        menu.push(Action::Confirm, Action::Confirm, "open");
        if !is_dir {
            menu.push(
                Action::CheckpointFile,
                Action::CheckpointFile,
                "checkpoint this file",
            );
        }
        // A file with listed threads offers its two thread views (ADR
        // 0066).
        if has_threads {
            menu.push(Action::ThreadsOnFile, Action::ThreadsOnFile, "threads");
            menu.push(Action::Review, Action::Review, "review");
        }
        menu.push(Action::CopyPath, Action::CopyPath, "copy path");
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

    /// Whether `place` groups its threads by file now, so a fold has
    /// something to fold (ADR 0066).
    fn folds_files(&self, place: Where) -> bool {
        match place {
            Where::ThreadsPane => self.sidebar_scope() == PaneScope::Workspace,
            Where::Review => !self.review().file_only,
            _ => false,
        }
    }

    /// The menu for a file row (ADR 0066): fold or unfold it, fold or
    /// unfold every file, open the file, and the resolved toggle.
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
        let any_folded = self.review_entries(false).iter().any(|entry| match place {
            Where::ThreadsPane => self.threads_pane_is_folded(entry.path()),
            _ => self.review_list().is_folded(entry.path()),
        });
        menu.push(
            Action::FoldAll,
            Action::FoldAll,
            if any_folded { "unfold all" } else { "fold all" },
        );
        menu.push(Action::Confirm, Action::Confirm, "open file");
        menu.push(
            Action::ReviewResolved,
            Action::ReviewResolved,
            if self.review().resolved {
                "hide resolved"
            } else {
                "show resolved"
            },
        );
        menu
    }

    /// The menu for the thread cursor's thread on a thread surface.
    fn thread_menu(&self, place: Where, column: usize, row: usize) -> Menu {
        let mut menu = Menu::new("thread", place, column, row);
        menu.push(Action::Confirm, Action::Confirm, "go to");
        menu.push(Action::Reply, Action::Reply, "reply");
        let resolved = self
            .thread_cursor()
            .thread()
            .and_then(|id| self.thread(id))
            .is_some_and(|thread| Words::of(None, thread).is_resolved());
        menu.push(
            Action::ToggleResolved,
            Action::ToggleResolved,
            if resolved {
                "reopen thread"
            } else {
                "resolve thread"
            },
        );
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
