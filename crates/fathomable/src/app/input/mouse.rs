// @okf-doc: /decisions/0073-the-chevron.md
//! The mouse goes to the pane under the pointer, not the focused one
//! (ADR 0007): the wheel scrolls what it is over, a click focuses it,
//! and a press on a border starts a drag. The right button opens the
//! context menu for what is under the pointer, the drawn key menus and
//! pane-header hints take clicks, and the gutter, a double- or
//! triple-click, and Shift-click select (ADR 0050). A click in a
//! thread's gutter or a double-click anywhere on its header or stub
//! toggles the thread; a single click elsewhere rests the cursor on
//! that row (ADR 0073).

use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::super::{App, Border, Focus, Popup};
use super::bindings::{self, Where};
use super::help;
use super::keys::{self, WHEEL_LINES};
use crate::app::draw;
use crate::app::draw::header;
use crate::app::threads::draft::DraftRow;
use crate::app::threads::list::Row;
use crate::app::view::Effect;
use crate::app::{doctor_view, licenses, menu_bar};

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
    app.sync_text_height();
    let effect = mouse_event(app, event);
    app.sync_text_height();
    effect
}

/// The mouse over the sidebar: the threads pane along its bottom (ADR 0027,
/// ADR 0049) takes what lands on it; the tree above pages the viewer.
fn sidebar_mouse(
    app: &mut App,
    kind: MouseEventKind,
    column: usize,
    row: usize,
    screen_row: usize,
) -> Effect {
    let tree_rows = app.tree_rows();
    if row >= tree_rows && row < app.pane_rows() && app.threads_pane_height() > 0 {
        return threads_pane_mouse(app, kind, column, screen_row, row - tree_rows);
    }
    match kind {
        MouseEventKind::ScrollDown => app.scroll_tree_by(WHEEL_LINES),
        MouseEventKind::ScrollUp => app.scroll_tree_by(-WHEEL_LINES),
        // Row 0 is the Files header.
        MouseEventKind::Down(MouseButton::Left) if row >= 1 => app.tree_click(row - 1),
        MouseEventKind::Down(MouseButton::Left)
            if column < header::files_pane_header(app).left_width() =>
        {
            app.focus_pane(Focus::Tree);
            app.open_files_menu(screen_row);
        }
        MouseEventKind::Down(MouseButton::Left) => app.focus_pane(Focus::Tree),
        MouseEventKind::Down(MouseButton::Right) if row >= 1 => {
            app.open_tree_menu(row - 1, column, screen_row);
        }
        _ => {}
    }
    Effect::None
}

/// The mouse over the threads pane (ADR 0027, ADR 0066): the wheel
/// steps between threads, a click on either row of a thread goes to it
/// and one on a file row folds it, a click on the header toggles the
/// title menu, a click on the key bar runs its hint, a right-click on a
/// row opens its menu, and the rule drags. Row 0 is the rule, row 1 the
/// header, and the last row is the key bar only while the pane has the keys.
fn threads_pane_mouse(
    app: &mut App,
    kind: MouseEventKind,
    column: usize,
    row: usize,
    pane_row: usize,
) -> Effect {
    let inner = app.sidebar_width().saturating_sub(1);
    let bar =
        app.pane_has_navigation(Focus::ThreadsPane) && pane_row + 1 == app.threads_pane_height();
    match kind {
        MouseEventKind::ScrollDown => app.scroll_threads_pane(WHEEL_LINES),
        MouseEventKind::ScrollUp => app.scroll_threads_pane(-WHEEL_LINES),
        MouseEventKind::Down(MouseButton::Left) if pane_row == 0 => {
            app.begin_drag(Border::ThreadsPane);
        }
        MouseEventKind::Down(MouseButton::Left) if pane_row == 1 => {
            app.threads_pane_focus();
            let header = header::threads_pane_header(app);
            if column < header.left_width() {
                app.open_threads_menu(row);
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

/// The mouse over the review list (ADR 0025, ADR 0059, ADR 0066, ADR
/// 0076): the wheel scrolls, a click selects the entry under the
/// pointer or folds the file row it lands on, a click on a thread's
/// chevron or a double-click on its row folds or expands it, a click on
/// the `Reviews` title opens its settings, a key-bar hint runs it, and a
/// right-click on a row opens its menu. Row 0 is the list header and the
/// column's last row is the key bar.
fn review_mouse(
    app: &mut App,
    kind: MouseEventKind,
    column: usize,
    row: usize,
    screen_row: usize,
) -> Effect {
    let footer_shown = app.focus() == Focus::Review;
    let bar_row = app.pane_rows().saturating_sub(1);
    let body_end = app.pane_rows().saturating_sub(usize::from(footer_shown));
    match kind {
        MouseEventKind::ScrollDown => app.review_scroll(WHEEL_LINES),
        MouseEventKind::ScrollUp => app.review_scroll(-WHEEL_LINES),
        MouseEventKind::Down(MouseButton::Left) if row == 0 => {
            app.focus_pane(Focus::Review);
            let local = column.saturating_sub(app.sidebar_width());
            let header = header::review_header(app);
            if header.control_at(app.column_width(), local) == Some(header::Control::DiffMode) {
                if let Some((_, end)) =
                    header.control_bounds(app.column_width(), header::Control::DiffMode)
                {
                    app.open_diff_mode_menu(app.sidebar_width() + end, screen_row);
                }
            } else if app.review().view == crate::app::threads::list::ReviewView::Board
                && local < header.left_width()
            {
                app.open_reviews_settings_menu(screen_row);
            }
        }
        MouseEventKind::Down(MouseButton::Left) if footer_shown && row == bar_row => {
            let rows = app.review_rows(app.column_width());
            let footer = header::review_footer(app, &rows.entries);
            if let Some(action) = footer.action_at(app.column_width(), column - app.sidebar_width())
            {
                return app.act(action);
            }
        }
        // A thread's header or folded row (ADR 0076): a press on the
        // chevron's cell, after the cursor cell and the nest (ADR
        // 0077), or a second press on any of its cells, folds or
        // expands the thread and ends the gesture, as in the text (ADR
        // 0073); one press elsewhere selects.
        MouseEventKind::Down(MouseButton::Left) => {
            let list_row = row - 1;
            let width = app.column_width();
            let rows = app.review_rows(width);
            let at = app.review_list().scroll() + list_row;
            let summary_row = rows.rows.get(at).and_then(|row| match row {
                Row::Header { entry, summary, .. } | Row::Stub { entry, summary, .. } => {
                    Some((*entry, summary))
                }
                _ => None,
            });
            if let Some((_entry, summary)) = summary_row {
                let local = column.saturating_sub(app.sidebar_width());
                let layout =
                    header::entry_header(summary, fathomable_core::clock::now(), false, width);
                if layout.disclosure_at(local) || press(app, column, screen_row, false) == 2 {
                    let id = summary.id().clone();
                    app.review_click(list_row);
                    app.review_toggle_thread(&id);
                    app.press = None;
                    return Effect::None;
                }
            }
            app.review_click(list_row);
        }
        MouseEventKind::Down(MouseButton::Right) if row >= 1 && row < body_end => {
            app.open_review_menu(row - 1, column, screen_row);
        }
        _ => {}
    }
    Effect::None
}

/// A click on the help popup (ADR 0050, ADR 0078): the binding on that
/// wrapped row runs when it applies on the focused surface. Other cells
/// keep help open.
fn help_click(app: &mut App, column: usize, row: usize) -> Effect {
    let binding = help::state(app).and_then(|help| {
        let (width, height) = app.size();
        help.layout(width, height).binding_at(column, row)
    });
    let Some(binding) = binding else {
        return Effect::None;
    };
    app.close_popup();
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
#[expect(
    clippy::too_many_lines,
    reason = "popup mouse precedence stays explicit in one ordered dispatcher"
)]
fn popup_mouse(app: &mut App, kind: MouseEventKind, column: usize, row: usize) -> Option<Effect> {
    let left = kind == MouseEventKind::Down(MouseButton::Left);
    let right = kind == MouseEventKind::Down(MouseButton::Right);
    // The context menu takes the mouse while it is open (ADR 0050): a
    // click on an entry runs it, a click elsewhere closes it, and a
    // right-click elsewhere closes it and opens the menu for there.
    if let Some(menu) = app.menu() {
        if left {
            let (width, _) = app.size();
            let grid = menu.grid_in(width, app.pane_top(), app.pane_rows());
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
        Some(Popup::DiffMode(mode)) => {
            let (width, _) = app.size();
            let grid = mode.menu().grid_in(width, app.pane_top(), app.pane_rows());
            match kind {
                MouseEventKind::Down(MouseButton::Left) if !grid.contains(column, row) => {
                    app.close_popup();
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(index) = grid.entry_at(column, row) {
                        return Some(app.mode_menu_click(index));
                    }
                }
                _ => {}
            }
            Some(Effect::None)
        }
        Some(Popup::Help(_)) => match kind {
            MouseEventKind::ScrollDown => Some(help::wheel(app, WHEEL_LINES)),
            MouseEventKind::ScrollUp => Some(help::wheel(app, -WHEEL_LINES)),
            _ if left => Some(help_click(app, column, row)),
            _ => Some(Effect::None),
        },
        Some(Popup::Status) if left => {
            app.close_popup();
            Some(Effect::None)
        }
        Some(Popup::Picker(picker)) => {
            let layout = draw::picker_layout(app, picker);
            let entry = layout.entry_at(column, row, picker.matched());
            match kind {
                MouseEventKind::ScrollDown if layout.contains(column, row) => {
                    app.picker_move(WHEEL_LINES);
                }
                MouseEventKind::ScrollUp if layout.contains(column, row) => {
                    app.picker_move(-WHEEL_LINES);
                }
                MouseEventKind::Down(MouseButton::Left) if !layout.contains(column, row) => {
                    app.close_popup();
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(index) = entry {
                        app.picker_select(index);
                        app.picker_confirm();
                    }
                }
                _ => {}
            }
            Some(Effect::None)
        }
        Some(Popup::Doctor(_)) => match kind {
            MouseEventKind::ScrollDown => Some(doctor_view::wheel(app, WHEEL_LINES)),
            MouseEventKind::ScrollUp => Some(doctor_view::wheel(app, -WHEEL_LINES)),
            _ if left => {
                if !inside(draw::report_area(app), column, row) {
                    app.close_popup();
                }
                Some(Effect::None)
            }
            _ => Some(Effect::None),
        },
        Some(Popup::Licenses(_)) => match kind {
            MouseEventKind::ScrollDown => Some(licenses::wheel(app, WHEEL_LINES)),
            MouseEventKind::ScrollUp => Some(licenses::wheel(app, -WHEEL_LINES)),
            _ if left => {
                if !inside(draw::report_area(app), column, row) {
                    app.close_popup();
                }
                Some(Effect::None)
            }
            _ => Some(Effect::None),
        },
        Some(Popup::About) if left => {
            let area = draw::about_area(app);
            let source_row = usize::from(area.y) + 7;
            let source_start = usize::from(area.x) + 10;
            let source_end = source_start + "https://github.com/hbeberman/fathomable".len();
            if row == source_row && column >= source_start && column < source_end {
                app.close_popup();
                Some(Effect::Open(
                    "https://github.com/hbeberman/fathomable".to_owned(),
                ))
            } else {
                if !inside(area, column, row) {
                    app.close_popup();
                }
                Some(Effect::None)
            }
        }
        Some(
            Popup::ConfirmQuit
            | Popup::ConfirmBoard { .. }
            | Popup::ConfirmReviewPointDelete { .. },
        ) => Some(confirmation_mouse(app, left, column, row)),
        Some(Popup::Status | Popup::About) => Some(Effect::None),
        _ => None,
    }
}

fn confirmation_mouse(app: &mut App, left: bool, column: usize, row: usize) -> Effect {
    if !left {
        return Effect::None;
    }
    let is_quit = matches!(app.popup(), Some(Popup::ConfirmQuit));
    let is_point = matches!(app.popup(), Some(Popup::ConfirmReviewPointDelete { .. }));
    let layout = match app.popup() {
        Some(Popup::ConfirmQuit) => draw::quit_confirmation_layout(app),
        Some(Popup::ConfirmBoard { changed, .. }) => draw::board_confirmation_layout(app, *changed),
        Some(Popup::ConfirmReviewPointDelete { .. }) => {
            draw::review_point_delete_confirmation_layout(app)
        }
        _ => return Effect::None,
    };
    match layout.action_at(column, row) {
        Some(bindings::Action::Confirm) if is_quit => {
            app.close_popup();
            Effect::Quit
        }
        Some(bindings::Action::Confirm) => {
            if is_point && app.review_point_delete_confirmation_armed() {
                app.confirm_review_point_delete();
            } else if !is_point {
                app.confirm_clear_board();
            }
            Effect::None
        }
        Some(bindings::Action::Escape) if is_quit => {
            app.close_popup();
            Effect::None
        }
        Some(bindings::Action::Escape) => {
            if is_point {
                app.cancel_review_point_delete();
            } else {
                app.cancel_clear_board();
            }
            Effect::None
        }
        _ if !layout.contains(column, row) => {
            if is_quit {
                app.close_popup();
            } else if is_point {
                app.cancel_review_point_delete();
            } else {
                app.cancel_clear_board();
            }
            Effect::None
        }
        _ => Effect::None,
    }
}

fn inside(area: ratatui::layout::Rect, column: usize, row: usize) -> bool {
    column >= usize::from(area.x)
        && column < usize::from(area.x + area.width)
        && row >= usize::from(area.y)
        && row < usize::from(area.y + area.height)
}

/// A click on a which-key entry is that key typed (ADR 0050).
fn which_key_click(app: &mut App, column: usize, row: usize) -> Option<Effect> {
    let place = keys::place(app).filter(|_| !app.prefix().is_empty())?;
    let entries = app.which_key_rows(place);
    let shown: Vec<(String, String)> = entries
        .iter()
        .map(super::bindings::MenuRow::display)
        .collect();
    let grid = draw::which_key_grid(app, &shown);
    let index = grid.entry_at(column, row)?;
    Some(match entries[index].chord() {
        Some(chord) => keys::typed(app, place, chord),
        None => Effect::None,
    })
}

fn mouse_event(app: &mut App, event: MouseEvent) -> Effect {
    let row = usize::from(event.row);
    let column = usize::from(event.column);
    app.pointer = Some((column, row));
    if !app.panes_fit() {
        app.end_drag();
        if let Some(effect) = menu_bar::mouse(app, event) {
            return effect;
        }
        return if matches!(app.popup(), Some(Popup::ConfirmQuit)) {
            popup_mouse(app, event.kind, column, row).unwrap_or(Effect::None)
        } else {
            Effect::None
        };
    }
    if app.dragging().is_some() {
        match event.kind {
            MouseEventKind::Drag(MouseButton::Left) => {
                app.drag_to(column, row.saturating_sub(app.pane_top()));
            }
            MouseEventKind::Up(MouseButton::Left) => app.end_drag(),
            _ => {}
        }
        return Effect::None;
    }
    if let Some(effect) = menu_bar::mouse(app, event) {
        return effect;
    }
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
    let pane_top = app.pane_top();
    let Some(pane_row) = row.checked_sub(pane_top) else {
        return Effect::None;
    };
    let sidebar = app.sidebar_width();
    if left && pane_row < rows && sidebar > 0 && column + 1 == sidebar {
        app.begin_drag(Border::Sidebar);
        return Effect::None;
    }
    // The draft keeps the keys, and the right button while it is open
    // (ADR 0050); the rest of the mouse works around it.
    if right && matches!(app.popup(), Some(Popup::Compose(_))) {
        return Effect::None;
    }
    // The status line's thread total takes a click.
    if left && pane_row == rows {
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
        return sidebar_mouse(app, event.kind, column, pane_row, row);
    }
    if app.getting_started() {
        return Effect::None;
    }
    if app.review_list().is_open() {
        return review_mouse(app, event.kind, column, pane_row, row);
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
#[expect(
    clippy::too_many_lines,
    reason = "Text mouse precedence is kept in one ordered dispatcher."
)]
fn text_mouse(app: &mut App, event: MouseEvent, column: usize, row: usize) -> Effect {
    let sidebar = app.sidebar_width();
    let gutter = sidebar + crate::app::draw::gutter_width(app.view());
    let in_gutter = column < gutter;
    let col = column.saturating_sub(gutter);
    let text_rows = app.text_rows();
    let left = event.kind == MouseEventKind::Down(MouseButton::Left);
    // The banner and file header take rows over the text.
    let top = app.text_top();
    let Some(text_row) = row.checked_sub(top) else {
        if app.file_chrome_rows() > 0 && row + 1 == top {
            if left {
                app.focus_pane(Focus::View);
                let header = header::file_header(app);
                let local = column.saturating_sub(sidebar);
                if header.control_at(app.column_width(), local) == Some(header::Control::DiffMode) {
                    if let Some((_, end)) =
                        header.control_bounds(app.column_width(), header::Control::DiffMode)
                    {
                        app.open_diff_mode_menu(sidebar + end, row);
                    }
                } else if local < header.title_width() {
                    app.open_file_menu(row);
                }
            }
            return Effect::None;
        }
        return Effect::None;
    };
    if app.directory_path().is_some() {
        if left {
            app.focus_pane(Focus::View);
        }
        return Effect::None;
    }
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
        // first press elsewhere on the row rests the cursor on the row
        // itself, the stub's or the header's, neither a row a motion
        // stops on (amended 2026-09-09 and 2026-09-10). Opening and
        // closing end the gesture, so a double-click does not undo
        // itself.
        if text_row < text_rows
            && let Some((stub, index, _)) = app.stub_on_row(app.view().scroll() + text_row)
            && let Some(id) = stub.thread().cloned()
            && (!stub.expanded() || index == 0)
        {
            let Some(thread) = app.thread(&id) else {
                return Effect::None;
            };
            let width = app
                .column_width()
                .saturating_sub(crate::app::draw::gutter_width(app.view()));
            let layout = header::expanded_header(app, thread, stub.expanded(), width);
            if (!in_gutter && layout.disclosure_at(col)) || press(app, column, row, in_gutter) == 2
            {
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
            } else {
                view.rest_on(text_row);
            }
            return Effect::None;
        }
        if text_row >= text_rows {
            return Effect::None;
        }
        // A Ctrl-click opens the linked file or URL (ADR 0052).
        if event.modifiers.contains(KeyModifiers::CONTROL) {
            app.view_mut().click(text_row, col);
            return app.act(bindings::Action::GotoFile);
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
