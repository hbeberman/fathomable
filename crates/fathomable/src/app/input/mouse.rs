// @okf-doc: /decisions/0073-the-chevron.md
//! The mouse goes to the pane under the pointer, not the focused one
//! (ADR 0007): the wheel scrolls what it is over, a click focuses it,
//! and a press on a border starts a drag. The right button opens the
//! context menu for what is under the pointer, the drawn key menus and
//! pane-header hints take clicks, and the gutter, a double- or
//! triple-click, and Shift-click select (ADR 0050). A click on a
//! stub's `▸` or a double-click on the stub expands its thread, and a
//! click on the expanded header's `▾` or a double-click on the header
//! folds it; one click on either places the cursor (ADR 0073).

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fathomable_core::layout::display_width;

use super::super::{App, Border, Focus, Popup};
use super::bindings::{self, Where};
use super::keys::{self, WHEEL_LINES, tree_highlight};
use crate::app::draw;
use crate::app::draw::author::THREAD_GUTTER;
use crate::app::draw::header;
use crate::app::threads::draft::DraftRow;
use crate::app::view::Effect;

/// Presses on one cell closer together than this are one gesture.
const MULTI_CLICK: Duration = Duration::from_millis(400);

/// The last left press in the text (ADR 0050): where and when, how many
/// in a row on that cell, and whether it began in the gutter, which
/// makes the drag that follows select whole lines.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Press {
    at: Instant,
    column: usize,
    row: usize,
    count: u8,
    gutter: bool,
}

/// Apply a mouse event to whichever pane it lands on: the wheel scrolls
/// the pane under the pointer, a click focuses it, and a press on the
/// sidebar's divider or the threads pane's rule drags that border.
pub(crate) fn handle_mouse(app: &mut App, event: MouseEvent) -> Effect {
    app.with_navigation_watch(|app| mouse_event(app, event))
}

/// The mouse over the sidebar: the threads pane along its bottom (ADR 0027,
/// ADR 0049) takes what lands on it; the tree above pages the viewer.
fn sidebar_mouse(app: &mut App, kind: MouseEventKind, column: usize, row: usize) -> Effect {
    let tree_rows = app.tree_rows();
    if row >= tree_rows && row < app.pane_rows() && app.threads_pane_height() > 0 {
        return threads_pane_mouse(app, kind, column, row, row - tree_rows);
    }
    match kind {
        // One row per tick, not `WHEEL_LINES`: each tick pages the main
        // pane to the next file (ADR 0023). The text and the threads pane
        // keep their three-line wheel.
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            let before = tree_highlight(app);
            app.with_tree(|tree, _| {
                if kind == MouseEventKind::ScrollDown {
                    tree.move_down(1);
                } else {
                    tree.move_up(1);
                }
                None
            });
            if tree_highlight(app) != before {
                app.show_highlight();
            }
        }
        // Row 0 is the root header.
        MouseEventKind::Down(MouseButton::Left) if row >= 1 => app.tree_click(row - 1),
        // A click on the branch opens the worktree picker (ADR 0070).
        MouseEventKind::Down(MouseButton::Left) if app.has_worktrees() => app.pick_worktree(),
        MouseEventKind::Down(MouseButton::Left) => app.focus_pane(Focus::Tree),
        MouseEventKind::Down(MouseButton::Right) if row >= 1 => {
            app.open_tree_menu(row - 1, column, row);
        }
        _ => {}
    }
    Effect::None
}

/// The mouse over the threads pane (ADR 0027, ADR 0066): the wheel
/// steps between threads, a click on either row of a thread goes to it
/// and one on a file row folds it, a click on the header toggles the
/// scope (or, on its resolved count, resolved threads), a click on the
/// key bar runs its hint, a right-click on a row opens its menu, and
/// the rule drags. Row 0 is the rule, row 1 the header, and the last
/// row the key bar while the pane has the keys.
fn threads_pane_mouse(
    app: &mut App,
    kind: MouseEventKind,
    column: usize,
    row: usize,
    pane_row: usize,
) -> Effect {
    let inner = app.sidebar_width().saturating_sub(1);
    let bar = app.focus() == Focus::ThreadsPane && pane_row + 1 == app.threads_pane_height();
    match kind {
        MouseEventKind::ScrollDown => app.threads_pane_move(1),
        MouseEventKind::ScrollUp => app.threads_pane_move(-1),
        MouseEventKind::Down(MouseButton::Left) if pane_row == 0 => {
            app.begin_drag(Border::ThreadsPane);
        }
        MouseEventKind::Down(MouseButton::Left) if pane_row == 1 => {
            app.threads_pane_focus();
            let header = header::threads_pane_header(app);
            match header.action_at(inner, column) {
                Some(action) => return app.act(action),
                None => app.threads_pane_toggle_scope(),
            }
        }
        MouseEventKind::Down(MouseButton::Left) if bar => {
            let footer = header::threads_pane_footer(app);
            if let Some(action) = footer.action_at(inner, column) {
                return app.act(action);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => app.threads_pane_click(pane_row - 2),
        MouseEventKind::Down(MouseButton::Right) if pane_row >= 2 && !bar => {
            app.open_threads_pane_menu(pane_row - 2, column, row);
        }
        _ => {}
    }
    Effect::None
}

/// The mouse over the review list (ADR 0025, ADR 0059, ADR 0066): the
/// wheel scrolls, a click selects the entry under the pointer or folds
/// the file row it lands on, a click on the header's resolved count, a
/// key-bar hint, or a hint on the cursor's thread header runs it, and
/// a right-click on a row opens its menu. Row 0 is the list header and
/// the column's last row is the key bar; unfocused, a click on either
/// focuses the list.
fn review_mouse(app: &mut App, kind: MouseEventKind, column: usize, row: usize) -> Effect {
    let bar = app.pane_rows().saturating_sub(1);
    match kind {
        MouseEventKind::ScrollDown => app.review_scroll(WHEEL_LINES),
        MouseEventKind::ScrollUp => app.review_scroll(-WHEEL_LINES),
        MouseEventKind::Down(MouseButton::Left) if row == 0 || row == bar => {
            if app.focus() != Focus::Review {
                app.focus_pane(Focus::Review);
                return Effect::None;
            }
            let width = app.column_width();
            let rows = app.review_rows(width);
            let header = if row == 0 {
                header::review_header(app)
            } else {
                header::review_footer(app, &rows.entries)
            };
            if let Some(action) = header.action_at(width, column - app.sidebar_width()) {
                return app.act(action);
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            app.review_click(row - 1);
        }
        MouseEventKind::Down(MouseButton::Right) if row >= 1 && row < bar => {
            app.open_review_menu(row - 1, column, row);
        }
        _ => {}
    }
    Effect::None
}

/// A click on a diff's header (ADR 0050, ADR 0060): the base name,
/// before the ` · `, opens the base picker and the target name the
/// target picker; the keys are on the bar (ADR 0069).
fn diff_header_click(app: &mut App, column: usize) -> Effect {
    let Some(text) = app.diff_header() else {
        return Effect::None;
    };
    app.focus_pane(Focus::View);
    let header = header::diff_header(&text);
    if column < header.left_width() {
        // The left part is ` ` then the header text.
        let split = text
            .find(" · ")
            .map(|byte| 1 + display_width(&text[..byte]));
        let action = match split {
            Some(split) if column > split + 1 => bindings::Action::DiffTarget,
            _ => bindings::Action::DiffBase,
        };
        return app.act(action);
    }
    Effect::None
}

/// A click on the help popup (ADR 0050): the binding on that row runs
/// when it applies on the focused surface; any click closes the popup.
fn help_click(app: &mut App, column: usize, row: usize) -> Effect {
    let rows = bindings::help_rows();
    let shown: Vec<(String, String)> = rows.iter().map(|(_, row)| row.clone()).collect();
    let grid = draw::help_grid(app, &shown);
    let index = grid.entry_at(column, row);
    app.close_popup();
    let Some((Some(binding), _)) = index.and_then(|index| rows.get(index)) else {
        return Effect::None;
    };
    let Some(place) = keys::place(app) else {
        return Effect::None;
    };
    if binding.place == place || (binding.place == Where::Any && place.takes_any()) {
        return app.act(binding.action);
    }
    Effect::None
}

/// Count this left press at `(column, row)` against the last one: a
/// second on the same cell within [`MULTI_CLICK`] is a double-click, a
/// third a triple; the count wraps so a fourth starts over.
fn press(app: &mut App, column: usize, row: usize, gutter: bool) -> u8 {
    let now = Instant::now();
    let count = match app.press {
        Some(last)
            if now.duration_since(last.at) < MULTI_CLICK
                && last.column == column
                && last.row == row =>
        {
            last.count % 3 + 1
        }
        _ => 1,
    };
    app.press = Some(Press {
        at: now,
        column,
        row,
        count,
        gutter,
    });
    count
}

/// The popups' share of the mouse: the context menu, the help, and the
/// status overlay take a click; `None` when the event goes on to the
/// panes (no popup, or a right-click that just closed the menu).
fn popup_mouse(app: &mut App, kind: MouseEventKind, column: usize, row: usize) -> Option<Effect> {
    let left = kind == MouseEventKind::Down(MouseButton::Left);
    let right = kind == MouseEventKind::Down(MouseButton::Right);
    // The context menu takes the mouse while it is open (ADR 0050): a
    // click on an entry runs it, a click elsewhere closes it, and a
    // right-click elsewhere closes it and opens the menu for there.
    if let Some(menu) = app.menu() {
        if left {
            let (width, height) = app.size();
            let grid = menu.grid(width, height);
            if let Some(index) = grid.entry_at(column, row) {
                return Some(app.menu_click(index));
            }
            app.close_popup();
            return Some(Effect::None);
        }
        if !right {
            return Some(Effect::None);
        }
        app.close_popup();
        return None;
    }
    match app.popup() {
        Some(Popup::Help) if left => Some(help_click(app, column, row)),
        Some(Popup::Status) if left => {
            app.close_popup();
            Some(Effect::None)
        }
        Some(Popup::Help | Popup::Status | Popup::Picker(_)) => Some(Effect::None),
        _ => None,
    }
}

/// A click on a which-key entry is that key typed (ADR 0050).
fn which_key_click(app: &mut App, column: usize, row: usize) -> Option<Effect> {
    let place = keys::place(app).filter(|_| !app.prefix().is_empty())?;
    let entries = app.which_key(place);
    let shown: Vec<(String, String)> = entries
        .iter()
        .map(|(chord, label)| (chord.to_string(), label.clone()))
        .collect();
    let grid = draw::which_key_grid(app, &shown);
    let index = grid.entry_at(column, row)?;
    Some(keys::typed(app, place, entries[index].0))
}

fn mouse_event(app: &mut App, event: MouseEvent) -> Effect {
    let row = usize::from(event.row);
    let column = usize::from(event.column);
    app.pointer = Some((column, row));
    if let Some(effect) = popup_mouse(app, event.kind, column, row) {
        return effect;
    }
    let left = event.kind == MouseEventKind::Down(MouseButton::Left);
    let right = event.kind == MouseEventKind::Down(MouseButton::Right);
    if left {
        if let Some(effect) = which_key_click(app, column, row) {
            return effect;
        }
        // A click ends a pending key sequence and an armed delete.
        app.take_prefix();
        app.cancel_delete();
    }
    let rows = app.pane_rows();
    let sidebar = app.sidebar_width();
    if app.dragging().is_some() {
        match event.kind {
            MouseEventKind::Drag(MouseButton::Left) => app.drag_to(column, row),
            MouseEventKind::Up(MouseButton::Left) => app.end_drag(),
            _ => {}
        }
        return Effect::None;
    }
    if left && row < rows && sidebar > 0 && column + 1 == sidebar {
        app.begin_drag(Border::Sidebar);
        return Effect::None;
    }
    // The draft keeps the keys, and the right button while it is open
    // (ADR 0050); the rest of the mouse works around it.
    if right && matches!(app.popup(), Some(Popup::Compose(_))) {
        return Effect::None;
    }
    // The status line's waiting and thread counts take a click (ADR
    // 0066).
    if left && row == rows {
        let parts = draw::status_parts(app);
        let start = app.size().0.saturating_sub(parts.right_width());
        if let Some(action) = column
            .checked_sub(start)
            .and_then(|column| parts.action_at(column))
        {
            return app.act(action);
        }
        return Effect::None;
    }
    if column < sidebar {
        return sidebar_mouse(app, event.kind, column, row);
    }
    if app.review_list().is_open() {
        return review_mouse(app, event.kind, column, row);
    }
    // The text's key bar over the bottom text row (ADR 0067): a click
    // focuses the text and runs the hint under the pointer.
    if left && app.text_bar_shown() && row == app.text_bar_row() {
        let bar = header_bar(app);
        app.focus_pane(Focus::View);
        if let Some(action) = bar.action_at(app.column_width(), column - sidebar) {
            return app.act(action);
        }
        return Effect::None;
    }
    text_mouse(app, event, column, row)
}

/// The text's key bar as drawn, for the click under the pointer.
fn header_bar(app: &App) -> header::Header {
    crate::app::draw::bar::text_bar(app)
}

/// The mouse over the text: the wheel scrolls; a right-click opens the
/// menu; a left press places the cursor, expands a stub, or begins a
/// selection gesture; a drag extends the selection.
fn text_mouse(app: &mut App, event: MouseEvent, column: usize, row: usize) -> Effect {
    let sidebar = app.sidebar_width();
    let gutter = sidebar + crate::app::draw::gutter_width(app.view());
    let in_gutter = column < gutter;
    let col = column.saturating_sub(gutter);
    let text_rows = app.text_rows();
    let left = event.kind == MouseEventKind::Down(MouseButton::Left);
    // The banner and the diff header take rows over the text.
    let top = app.text_top();
    let Some(text_row) = row.checked_sub(top) else {
        if left && app.diff_chrome_rows() > 0 && row + 1 == top {
            return diff_header_click(app, column - sidebar);
        }
        return Effect::None;
    };
    if event.kind == MouseEventKind::Down(MouseButton::Right) {
        if text_row < text_rows {
            app.open_view_menu(text_row, col, column, row);
        }
        return Effect::None;
    }
    if left {
        // A click on the draft places its cursor, the keys staying where
        // they were (ADR 0054); its author row takes nothing.
        if text_row < text_rows
            && let Some(draft_row) = app.draft_row_of(app.view().scroll() + text_row)
        {
            if let DraftRow::Text(index) = draft_row {
                app.draft_place_cursor(index, col);
            }
            return Effect::None;
        }
        app.focus_pane(Focus::View);
        // A stub's rows and the expanded header (ADR 0073): a press on
        // the thread's gutter, the chevron's column, opens or closes
        // the thread, as does a second press on any of its cells; a
        // first press elsewhere on the row places the cursor, on a
        // collapsed stub's own row (amended 2026-09-09). Opening and
        // closing end the gesture, so a double-click does not undo
        // itself.
        if text_row < text_rows
            && let Some((stub, index, _)) = app.stub_on_row(app.view().scroll() + text_row)
            && let Some(id) = stub.thread().cloned()
            && (!stub.expanded() || index == 0)
        {
            let chevron = !in_gutter && col < THREAD_GUTTER;
            if chevron || press(app, column, row, in_gutter) == 2 {
                if stub.expanded() {
                    app.fold_thread(&id);
                } else {
                    let newest = app.newest_message(&id);
                    app.goto_message(id, newest);
                }
                app.press = None;
                return Effect::None;
            }
            let view = app.view_mut();
            view.touch();
            if in_gutter {
                view.select_line_at(text_row);
            } else if stub.expanded() {
                view.click(text_row, col);
            } else {
                view.rest_on(text_row);
            }
            return Effect::None;
        }
        if text_row >= text_rows {
            return Effect::None;
        }
        // A Ctrl-click opens the file named under the pointer (ADR 0052).
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            app.view_mut().click(text_row, col);
            app.act(bindings::Action::GotoFile);
            return Effect::None;
        }
        let count = press(app, column, row, in_gutter);
        let shift = event.modifiers.contains(KeyModifiers::SHIFT);
        let view = app.view_mut();
        view.touch();
        if shift {
            view.extend_to(text_row, col);
        } else if in_gutter || count == 3 {
            view.select_line_at(text_row);
        } else if count == 2 {
            view.select_word_at(text_row, col);
        } else {
            view.click(text_row, col);
        }
        return Effect::None;
    }
    let linewise = app.press.is_some_and(|press| press.gutter);
    let view = app.view_mut();
    view.touch();
    match event.kind {
        MouseEventKind::ScrollDown => view.scroll_by(WHEEL_LINES),
        MouseEventKind::ScrollUp => view.scroll_by(-WHEEL_LINES),
        MouseEventKind::Drag(MouseButton::Left) => {
            let text_row = text_row.min(text_rows.saturating_sub(1));
            if linewise {
                view.drag_lines(text_row);
            } else {
                view.drag(text_row, col);
            }
        }
        MouseEventKind::Up(MouseButton::Left) => view.release(),
        _ => {}
    }
    Effect::None
}
