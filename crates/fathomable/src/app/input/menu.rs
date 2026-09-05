// @okf-doc: /decisions/0050-mouse-menus-and-gestures.md
//! The context menu a right-click opens (ADR 0050): the actions that
//! apply under the pointer, each showing the key the binding table
//! gives it, and the grids that the drawn menus and the mouse share so
//! a click lands on the entry that was drawn there.
//!
//! A [`Menu`] is one more [`Popup`]. Its entries act on the cursor the
//! right-click placed, so they are ordinary [`Action`]s run through
//! [`App::act`]; the menu needs no target of its own.

use super::bindings::{self, Action, Chord, Keys, Match, Where};
use crate::app::threads::words::Words;
use crate::app::view::{Effect, Mode};
use crate::app::{App, Focus, Popup};
use fathomable_core::layout::display_width;
use fathomable_core::tree::Tree;

/// One row of a context menu.
#[derive(Debug, Clone)]
pub struct Entry {
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
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    #[must_use]
    pub fn action(&self) -> Action {
        self.action
    }
}

/// A context menu: what it acts on, its entries, and the cell it opened
/// at.
#[derive(Debug, Clone)]
pub struct Menu {
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
    pub fn title(&self) -> &str {
        &self.title
    }

    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// What `typed` means here: an entry's whole key fires it, the start
    /// of one waits, anything else is a miss that closes the menu.
    #[must_use]
    pub fn typed(&self, typed: &[Chord]) -> Match {
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
    pub fn grid(&self, width: usize, height: usize) -> Grid {
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
pub struct Grid {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub rows: usize,
    pub columns: usize,
    pub key_width: usize,
    pub label_width: usize,
    pub count: usize,
}

impl Grid {
    /// The cells one column of entries takes.
    #[must_use]
    pub fn column_width(key_width: usize, label_width: usize) -> usize {
        key_width + 2 + label_width + 3
    }

    /// The which-key menu (ADR 0045) along the bottom of the pane at
    /// `(x, y)` of `pane_width` by `pane_height`: as many columns as
    /// keep the box to eight rows.
    #[must_use]
    pub fn bottom(
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
    pub fn centred(
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
    pub fn contains(&self, column: usize, row: usize) -> bool {
        column >= self.x
            && column < self.x + self.width
            && row >= self.y
            && row < self.y + self.height
    }

    /// The entry drawn at the cell, if any.
    #[must_use]
    pub fn entry_at(&self, column: usize, row: usize) -> Option<usize> {
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
    pub fn menu(&self) -> Option<&Menu> {
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
                Action::Comment,
                Action::Comment,
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
        let mut menu = Menu::new(current.name().to_owned(), Where::Tree, column, row);
        menu.push(Action::Confirm, Action::Confirm, "open");
        if !is_dir {
            menu.push(
                Action::CheckpointFile,
                Action::CheckpointFile,
                "checkpoint this file",
            );
        }
        menu.push(Action::CopyPath, Action::CopyPath, "copy path");
        menu.push(Action::TreeRefresh, Action::TreeRefresh, "re-read the tree");
        menu.push(Action::TreeIgnored, Action::TreeIgnored, "toggle ignored");
        self.open_menu(menu);
    }

    /// A right-click on a threads pane entry at screen `(column, row)`:
    /// the thread cursor moves there, then the thread's menu.
    pub(super) fn open_threads_pane_menu(&mut self, entry_row: usize, column: usize, row: usize) {
        self.threads_pane_click(entry_row);
        if self.thread_cursor().thread().is_some() {
            let menu = self.thread_menu(Where::ThreadsPane, column, row);
            self.open_menu(menu);
        }
    }

    /// A right-click on a review list row at screen `(column, row)`.
    pub(super) fn open_review_menu(&mut self, list_row: usize, column: usize, row: usize) {
        self.review_click(list_row);
        if self.thread_cursor().thread().is_some() {
            let menu = self.thread_menu(Where::Review, column, row);
            self.open_menu(menu);
        }
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
        menu
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Context as _;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use fathomable_core::tree::Tree;
    use fathomable_testing::TempDir;

    use crate::app::testing::{self, app, key};

    use super::super::bindings::{self, Action, Where};
    use super::super::keys::handle_key;
    use super::super::mouse::handle_mouse;
    use super::Menu;
    use crate::app::draw;
    use crate::app::threads::ComposeTarget;
    use crate::app::view::{Effect, Mode};
    use crate::app::{App, Focus, PickerKind, Popup};

    const LINK: &str = "https://example.com/guide";

    fn fixture(name: &str) -> anyhow::Result<TempDir> {
        let readme = format!("# Readme\n\nalpha beta\n\ngamma\n\nsee [the guide]({LINK})\n");
        let dir = testing::workspace(&format!("menu-{name}"), &readme)?;
        fs::create_dir_all(dir.0.join("ws/docs"))?;
        fs::write(dir.0.join("ws/docs/guide.md"), "# Guide\n")?;
        Ok(dir)
    }

    fn mouse(app: &mut App, kind: MouseEventKind, column: usize, row: usize) -> Effect {
        handle_mouse(
            app,
            MouseEvent {
                kind,
                column: u16::try_from(column).unwrap_or(u16::MAX),
                row: u16::try_from(row).unwrap_or(u16::MAX),
                modifiers: KeyModifiers::NONE,
            },
        )
    }

    fn left(app: &mut App, column: usize, row: usize) -> Effect {
        mouse(app, MouseEventKind::Down(MouseButton::Left), column, row)
    }

    fn right(app: &mut App, column: usize, row: usize) -> Effect {
        mouse(app, MouseEventKind::Down(MouseButton::Right), column, row)
    }

    /// The rendered row whose text contains `text`.
    fn row_of(app: &App, text: &str) -> anyhow::Result<usize> {
        app.view()
            .layout()
            .lines()
            .iter()
            .position(|line| line.text().contains(text))
            .with_context(|| format!("no row says {text:?}"))
    }

    /// The screen column of text column `col` and the screen row of
    /// rendered row `row`, with the view unscrolled.
    fn at(app: &App, row: usize, col: usize) -> (usize, usize) {
        let gutter = app.rail_width() + draw::gutter_width(app.view());
        (gutter + col, app.text_top() + row)
    }

    fn entries(app: &App) -> anyhow::Result<Vec<(String, String)>> {
        Ok(app
            .menu()
            .context("a menu is open")?
            .entries()
            .iter()
            .map(|entry| (entry.key().to_owned(), entry.label().to_owned()))
            .collect())
    }

    /// The screen cell of the open menu's entry labelled `label`.
    fn entry_cell(app: &App, label: &str) -> anyhow::Result<(usize, usize)> {
        let menu = app.menu().context("a menu is open")?;
        let index = menu
            .entries()
            .iter()
            .position(|entry| entry.label() == label)
            .with_context(|| format!("no entry {label:?} in {:?}", entries(app).ok()))?;
        let (width, height) = app.size();
        let grid = menu.grid(width, height);
        Ok((grid.x + 1, grid.y + 1 + index))
    }

    fn type_in(app: &mut App, text: &str) {
        for ch in text.chars() {
            app.compose_insert(&ch.to_string());
        }
    }

    /// A thread on the `alpha beta` line.
    fn annotate(app: &mut App) -> anyhow::Result<()> {
        let row = row_of(app, "alpha beta")?;
        app.view_mut().goto_row(row);
        app.view_mut().select_lines();
        app.start_comment();
        type_in(app, "look here");
        app.compose_submit();
        anyhow::ensure!(app.thread_counts().1 == 1, "thread not created");
        Ok(())
    }

    #[test]
    fn right_click_on_a_selection_keeps_it_and_the_menu_acts_on_it() -> anyhow::Result<()> {
        let dir = fixture("selection")?;
        let mut app = app(&dir)?;
        let row = row_of(&app, "alpha beta")?;
        let (column, screen_row) = at(&app, row, 3);
        // A press in the gutter selects the whole line.
        let rail = app.rail_width();
        left(&mut app, rail, screen_row);
        assert_eq!(app.view().mode(), Mode::Select);
        assert_eq!(app.view().selected_source().as_deref(), Some("alpha beta"));

        right(&mut app, column, screen_row);
        assert_eq!(app.view().mode(), Mode::Select, "the selection stays");
        assert_eq!(app.menu().map(Menu::title), Some("selection"));
        let keys: Vec<String> = entries(&app)?.into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["c", "C", "y", "Esc"]);

        // Hover is read from the pointer; a click on an entry runs it.
        let (x, y) = entry_cell(&app, "copy selection")?;
        mouse(&mut app, MouseEventKind::Moved, x, y);
        let (width, height) = app.size();
        let grid = app.menu().context("a menu is open")?.grid(width, height);
        assert_eq!(grid.entry_at(x, y), Some(2));
        assert_eq!(left(&mut app, x, y), Effect::Copy("alpha beta".to_owned()));
        assert!(app.menu().is_none(), "the menu closes once an entry runs");

        // Typing the entry's key runs it too.
        left(&mut app, rail, screen_row);
        right(&mut app, column, screen_row);
        handle_key(&mut app, key('c'));
        assert!(
            matches!(app.popup(), Some(Popup::Compose(_))),
            "c comments on the selection"
        );
        Ok(())
    }

    #[test]
    fn right_click_outside_the_selection_moves_the_cursor_and_offers_the_line() -> anyhow::Result<()>
    {
        let dir = fixture("line")?;
        let mut app = app(&dir)?;
        let alpha = row_of(&app, "alpha beta")?;
        let gamma = row_of(&app, "gamma")?;
        app.view_mut().goto_row(alpha);
        handle_key(&mut app, key('x'));
        assert_eq!(app.view().mode(), Mode::Select);

        let (column, screen_row) = at(&app, gamma, 1);
        right(&mut app, column, screen_row);
        assert_eq!(
            app.view().mode(),
            Mode::Normal,
            "a right-click outside clears the selection"
        );
        assert_eq!(app.view().cursor().row, gamma);
        assert_eq!(app.menu().map(Menu::title), Some("line 5"));
        assert_eq!(
            entries(&app)?,
            [
                ("c".to_owned(), "comment on line".to_owned()),
                ("x".to_owned(), "select line".to_owned()),
                ("y".to_owned(), "copy line".to_owned()),
            ]
        );

        // Esc closes and is swallowed; a click elsewhere closes too.
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.menu().is_none());
        right(&mut app, column, screen_row);
        let (far_column, far_row) = at(&app, alpha, 0);
        left(&mut app, far_column, far_row);
        assert!(app.menu().is_none());
        assert_eq!(
            app.view().cursor().row,
            gamma,
            "the closing click is swallowed"
        );

        // A key that is no entry closes the menu without acting.
        right(&mut app, column, screen_row);
        handle_key(&mut app, key('j'));
        assert!(app.menu().is_none());
        assert_eq!(app.view().cursor().row, gamma);

        // `y` from the menu copies the line.
        right(&mut app, column, screen_row);
        assert_eq!(
            handle_key(&mut app, key('y')),
            Effect::Copy("gamma".to_owned())
        );
        Ok(())
    }

    #[test]
    fn the_thread_menu_replies_and_deletes_at_once() -> anyhow::Result<()> {
        let dir = fixture("thread")?;
        let mut app = app(&dir)?;
        annotate(&mut app)?;
        let row = row_of(&app, "alpha beta")?;
        let (column, screen_row) = at(&app, row, 2);
        right(&mut app, column, screen_row);
        assert_eq!(app.menu().map(Menu::title), Some("thread"));
        let labels: Vec<String> = entries(&app)?.into_iter().map(|(_, l)| l).collect();
        assert_eq!(
            labels,
            [
                "expand thread",
                "reply",
                "resolve thread",
                "edit message",
                "delete thread",
                "new thread on line",
                "select line",
                "copy line",
            ]
        );
        let delete = entries(&app)?
            .into_iter()
            .find(|(_, l)| l == "delete thread");
        assert_eq!(
            delete.map(|(k, _)| k).as_deref(),
            Some("dd"),
            "the entry shows dd"
        );

        let (x, y) = entry_cell(&app, "reply")?;
        left(&mut app, x, y);
        assert!(matches!(
            app.popup(),
            Some(Popup::Compose(compose)) if matches!(compose.target(), ComposeTarget::Reply(_))
        ));
        app.compose_cancel();

        right(&mut app, column, screen_row);
        let (x, y) = entry_cell(&app, "delete thread")?;
        left(&mut app, x, y);
        assert_eq!(app.thread_counts().1, 0, "one click deletes");
        Ok(())
    }

    #[test]
    fn the_which_key_menu_and_the_help_take_clicks() -> anyhow::Result<()> {
        let dir = fixture("which-key")?;
        let mut app = app(&dir)?;
        handle_key(&mut app, key(' '));
        assert!(!app.prefix().is_empty());
        let shown = bindings::menu(Where::View, app.prefix());
        let index = shown
            .iter()
            .position(|(k, _)| k == "f")
            .context("Space f")?;
        let grid = draw::which_key_grid(&app, &shown);
        let cell = (0..grid.width)
            .flat_map(|x| (0..grid.height).map(move |y| (x, y)))
            .map(|(x, y)| (grid.x + x, grid.y + y))
            .find(|&(x, y)| grid.entry_at(x, y) == Some(index))
            .context("the entry is drawn somewhere")?;
        left(&mut app, cell.0, cell.1);
        assert!(matches!(app.popup(), Some(Popup::Picker(p)) if p.kind() == PickerKind::Files));
        assert!(app.prefix().is_empty());
        app.close_popup();

        // A click on a which-key entry that leads into a submenu descends.
        handle_key(&mut app, key(' '));
        let shown = bindings::menu(Where::View, app.prefix());
        let index = shown
            .iter()
            .position(|(k, _)| k == "v")
            .context("Space v")?;
        let grid = draw::which_key_grid(&app, &shown);
        let cell = (0..grid.width)
            .flat_map(|x| (0..grid.height).map(move |y| (x, y)))
            .map(|(x, y)| (grid.x + x, grid.y + y))
            .find(|&(x, y)| grid.entry_at(x, y) == Some(index))
            .context("the entry is drawn somewhere")?;
        left(&mut app, cell.0, cell.1);
        assert_eq!(bindings::spell(app.prefix()), "Space v");
        handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        // The help runs the binding on the clicked row when it applies here.
        app.open_help();
        let rows = bindings::help_rows();
        let index = rows
            .iter()
            .position(|(b, _)| b.is_some_and(|b| b.action == Action::MoveDown))
            .context("j is listed")?;
        let cursor_row = app.view().cursor().row;
        let shown: Vec<(String, String)> = rows.iter().map(|(_, r)| r.clone()).collect();
        let grid = draw::help_grid(&app, &shown);
        let cell = (0..grid.width)
            .flat_map(|x| (0..grid.height).map(move |y| (x, y)))
            .map(|(x, y)| (grid.x + x, grid.y + y))
            .find(|&(x, y)| grid.entry_at(x, y) == Some(index))
            .context("the row is drawn")?;
        left(&mut app, cell.0, cell.1);
        assert!(app.popup().is_none(), "the help closes");
        assert_eq!(app.view().cursor().row, cursor_row + 1, "j ran on the text");
        Ok(())
    }

    #[test]
    fn gutter_double_and_triple_clicks_select() -> anyhow::Result<()> {
        let dir = fixture("gestures")?;
        let mut app = app(&dir)?;
        let alpha = row_of(&app, "alpha beta")?;
        let gamma = row_of(&app, "gamma")?;
        let rail = app.rail_width();
        let (_, alpha_row) = at(&app, alpha, 0);
        let (_, gamma_row) = at(&app, gamma, 0);
        left(&mut app, rail, alpha_row);
        assert_eq!(app.view().selected_source().as_deref(), Some("alpha beta"));
        mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            rail,
            gamma_row,
        );
        mouse(
            &mut app,
            MouseEventKind::Up(MouseButton::Left),
            rail,
            gamma_row,
        );
        assert_eq!(
            app.view().selected_source().as_deref(),
            Some("alpha beta\n\ngamma")
        );
        assert_eq!(app.view().mode(), Mode::Select);

        let (beta, _) = at(&app, alpha, 7);
        left(&mut app, beta, alpha_row);
        assert_eq!(
            app.view().mode(),
            Mode::Normal,
            "one click places the cursor"
        );
        left(&mut app, beta, alpha_row);
        assert_eq!(
            app.view().selected_source().as_deref(),
            Some("beta"),
            "two select the word"
        );
        left(&mut app, beta, alpha_row);
        assert_eq!(
            app.view().selected_source().as_deref(),
            Some("alpha beta"),
            "three the line"
        );
        Ok(())
    }

    #[test]
    fn the_tree_menu_opens_and_copies_the_path() -> anyhow::Result<()> {
        let dir = fixture("tree")?;
        let mut app = app(&dir)?;
        app.show_tree();
        let index = app
            .tree()
            .map(Tree::rows)
            .and_then(|rows| rows.iter().position(|row| row.name() == "README.md"))
            .context("README.md is listed")?;
        right(&mut app, 0, index + 1);
        assert_eq!(app.focus(), Focus::Tree);
        assert_eq!(app.menu().map(Menu::title), Some("README.md"));
        let labels: Vec<String> = entries(&app)?.into_iter().map(|(_, l)| l).collect();
        assert_eq!(
            labels,
            [
                "open",
                "checkpoint this file",
                "copy path",
                "re-read the tree",
                "toggle ignored"
            ]
        );
        let (x, y) = entry_cell(&app, "copy path")?;
        assert_eq!(left(&mut app, x, y), Effect::Copy("README.md".to_owned()));
        assert_eq!(
            handle_key(&mut app, key('y')),
            Effect::Copy("README.md".to_owned())
        );
        Ok(())
    }

    #[test]
    fn links_copy_and_open_from_the_keys_and_the_menu() -> anyhow::Result<()> {
        let dir = fixture("links")?;
        let mut app = app(&dir)?;
        let row = row_of(&app, "the guide")?;
        let col = app.view().layout().lines()[row]
            .text()
            .find("the guide")
            .context("the link text")?;
        app.view_mut().goto_row(row);
        assert_eq!(handle_key(&mut app, key('g')), Effect::None);
        assert_eq!(
            handle_key(&mut app, key('y')),
            Effect::None,
            "the cursor sits before the link"
        );
        let (column, screen_row) = at(&app, row, col);
        left(&mut app, column, screen_row);
        assert_eq!(app.view().link_at_cursor(), Some(LINK));
        handle_key(&mut app, key('g'));
        assert_eq!(
            handle_key(&mut app, key('y')),
            Effect::Copy(LINK.to_owned())
        );
        handle_key(&mut app, key('g'));
        assert_eq!(
            handle_key(&mut app, key('x')),
            Effect::Open(LINK.to_owned())
        );

        right(&mut app, column, screen_row);
        let keys: Vec<(String, String)> = entries(&app)?;
        assert!(keys.contains(&("gy".to_owned(), "copy link".to_owned())));
        assert!(keys.contains(&("gx".to_owned(), "open link".to_owned())));
        // A two-key entry waits for its second key.
        assert_eq!(handle_key(&mut app, key('g')), Effect::None);
        assert!(app.menu().is_some());
        assert_eq!(
            handle_key(&mut app, key('x')),
            Effect::Open(LINK.to_owned())
        );
        assert!(app.menu().is_none());
        Ok(())
    }

    #[test]
    fn header_hints_take_clicks() -> anyhow::Result<()> {
        let dir = fixture("hints")?;
        let mut app = app(&dir)?;
        annotate(&mut app)?;

        // The expanded thread's header: the `reply` hint.
        let row = row_of(&app, "alpha beta")?;
        app.view_mut().goto_row(row);
        handle_key(&mut app, key('c'));
        let header_row = (0..app.view().layout().lines().len())
            .find(|&r| {
                app.stub_on_row(r)
                    .is_some_and(|(stub, index, _)| stub.expanded() && index == 0)
            })
            .context("the thread is expanded")?;
        let (stub, _, _) = app.stub_on_row(header_row).context("a stub")?;
        let thread = app.thread(stub.id()).context("the thread")?;
        let header = draw::expanded_header(&app, &stub, thread);
        let width = app.view().layout().width();
        let col = (0..width)
            .find(|&c| header.action_at(width, c) == Some(Action::Reply))
            .context("reply is drawn")?;
        let (column, screen_row) = at(&app, header_row - app.view().scroll(), col);
        left(&mut app, column, screen_row);
        assert!(matches!(
            app.popup(),
            Some(Popup::Compose(compose)) if matches!(compose.target(), ComposeTarget::Reply(_))
        ));
        // The comment box's own header: `Esc` cancels.
        let rows = app.pane_rows();
        let box_top = rows - app.compose_rows();
        let compose = match app.popup() {
            Some(Popup::Compose(compose)) => draw::compose_header(&app, compose),
            _ => unreachable!(),
        };
        let width = app.column_width();
        let col = (0..width)
            .find(|&c| compose.action_at(width, c) == Some(Action::Escape))
            .context("Esc is drawn")?;
        let rail = app.rail_width();
        left(&mut app, rail + col, box_top + 1);
        assert!(app.popup().is_none(), "the box closed");

        // The review list's header: the `sort` hint toggles the order.
        app.toggle_review();
        assert_eq!(app.focus(), Focus::Review);
        let list_rows = app.review_rows(app.column_width());
        let header = draw::review_header(&app, &list_rows.entries);
        let col = (0..width)
            .find(|&c| header.action_at(width, c) == Some(Action::ReviewSort))
            .context("sort is drawn")?;
        let before = app.review().sort;
        left(&mut app, rail + col, 0);
        assert_ne!(app.review().sort, before, "the sort hint ran");
        Ok(())
    }

    #[test]
    fn the_threads_pane_header_toggles_the_reach() -> anyhow::Result<()> {
        let dir = fixture("pane")?;
        let mut app = app(&dir)?;
        app.show_tree();
        if app.threads_pane_height() == 0 {
            app.toggle_threads_pane_shown();
        }
        let before = app.rail_scope();
        let header_row = app.tree_rows() + 1;
        left(&mut app, 1, header_row);
        assert_ne!(app.rail_scope(), before);
        assert_eq!(app.focus(), Focus::ThreadsPane);
        Ok(())
    }

    #[test]
    fn the_context_menu_draws() -> anyhow::Result<()> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let dir = fixture("draw")?;
        let mut app = app(&dir)?;
        let row = row_of(&app, "gamma")?;
        let (column, screen_row) = at(&app, row, 0);
        right(&mut app, column, screen_row);
        let (x, y) = entry_cell(&app, "select line")?;
        mouse(&mut app, MouseEventKind::Moved, x, y);
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = draw::Theme::from_core(&core);
        let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
        terminal.draw(|frame| draw::draw(frame, &app, &theme))?;
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol().to_owned())
            .collect::<String>();
        assert!(text.contains("line 5"), "the title names the line");
        assert!(text.contains("comment on line"));
        assert!(text.contains("select line"));
        Ok(())
    }
}
