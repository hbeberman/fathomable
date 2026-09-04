//! The mouse goes to the pane under the pointer, not the focused one
//! (ADR 0007): the wheel scrolls what it is over, a click focuses it,
//! and a press on a border starts a drag.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::super::{App, Border, Focus, Popup};
use super::keys::{WHEEL_LINES, tree_highlight};
use crate::app::view::Effect;

/// Apply a mouse event to whichever pane it lands on: the wheel scrolls
/// the pane under the pointer, a click focuses it, and a press on the
/// rail's divider or the threads pane's rule drags that border.
pub fn handle_mouse(app: &mut App, event: MouseEvent) -> Effect {
    app.with_navigation_watch(|app| mouse_event(app, event))
}

/// The mouse over the rail: the threads pane along its bottom (ADR 0027,
/// ADR 0049) takes what lands on it; the tree above pages the viewer.
fn sidebar_mouse(app: &mut App, kind: MouseEventKind, row: usize) {
    let tree_rows = app.tree_rows();
    if row >= tree_rows && row < app.pane_rows() && app.threads_pane_height() > 0 {
        threads_pane_mouse(app, kind, row - tree_rows);
        return;
    }
    match kind {
        // One row per tick, not `WHEEL_LINES`: each tick pages the main
        // pane to the next file (ADR 0023). The text and thread panes
        // below keep their three-line wheel.
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
        MouseEventKind::Down(MouseButton::Left) if row >= 1 => app.sidebar_click(row - 1),
        MouseEventKind::Down(MouseButton::Left) => app.focus_pane(Focus::Sidebar),
        _ => {}
    }
}

/// The mouse over the threads pane (ADR 0027): the wheel steps between
/// threads, a click on an entry goes to it, and the rule drags. Row 0 is
/// the rule, row 1 the header.
fn threads_pane_mouse(app: &mut App, kind: MouseEventKind, row: usize) {
    match kind {
        MouseEventKind::ScrollDown => app.threads_pane_move(1),
        MouseEventKind::ScrollUp => app.threads_pane_move(-1),
        MouseEventKind::Down(MouseButton::Left) if row == 0 => app.begin_drag(Border::ThreadsPane),
        MouseEventKind::Down(MouseButton::Left) if row >= 2 => app.threads_pane_click(row - 2),
        MouseEventKind::Down(MouseButton::Left) => app.threads_pane_focus(),
        _ => {}
    }
}

/// The mouse over the thread list (ADR 0025): the wheel scrolls, a click
/// selects the entry under the pointer. Row 0 is the list header.
fn thread_list_mouse(app: &mut App, kind: MouseEventKind, row: usize) {
    match kind {
        MouseEventKind::ScrollDown => app.thread_list_scroll(WHEEL_LINES),
        MouseEventKind::ScrollUp => app.thread_list_scroll(-WHEEL_LINES),
        MouseEventKind::Down(MouseButton::Left) if row >= 1 => app.thread_list_click(row - 1),
        _ => {}
    }
}

fn mouse_event(app: &mut App, event: MouseEvent) -> Effect {
    // The comment box keeps the keys but not the mouse: the reader can
    // scroll, click, and resize around it while writing.
    if matches!(
        app.popup(),
        Some(Popup::Help | Popup::Status | Popup::Picker(_))
    ) {
        return Effect::None;
    }
    if event.kind == MouseEventKind::Down(MouseButton::Left) {
        // A click ends a pending key sequence and an armed delete.
        app.take_prefix();
        app.cancel_delete();
    }
    let row = usize::from(event.row);
    let column = usize::from(event.column);
    let rows = app.pane_rows();
    let sidebar = app.rail_width();
    let box_rows = app.compose_rows();
    let box_top = rows.saturating_sub(box_rows);
    if app.dragging().is_some() {
        match event.kind {
            MouseEventKind::Drag(MouseButton::Left) => app.drag_to(column, row),
            MouseEventKind::Up(MouseButton::Left) => app.end_drag(),
            _ => {}
        }
        return Effect::None;
    }
    if event.kind == MouseEventKind::Down(MouseButton::Left) && row < rows {
        if sidebar > 0 && column + 1 == sidebar {
            app.begin_drag(Border::Sidebar);
            return Effect::None;
        }
        if box_rows > 0 && column >= sidebar && row == box_top {
            app.begin_drag(Border::Compose);
            return Effect::None;
        }
    }
    if box_rows > 0 && column >= sidebar && row > box_top && row < rows {
        // Rule and header, then the text rows.
        if event.kind == MouseEventKind::Down(MouseButton::Left) && row >= box_top + 2 {
            app.compose_click(row - box_top - 2, column - sidebar);
        }
        return Effect::None;
    }
    if column < sidebar {
        sidebar_mouse(app, event.kind, row);
        return Effect::None;
    }
    if app.thread_list().is_open() {
        thread_list_mouse(app, event.kind, row);
        return Effect::None;
    }
    let gutter = sidebar + crate::app::draw::gutter_width(app.view());
    let col = column.saturating_sub(gutter);
    let text_rows = app.text_rows();
    // The banner and the checkpoint header take rows over the text.
    let Some(row) = row.checked_sub(app.text_top()) else {
        return Effect::None;
    };
    if event.kind == MouseEventKind::Down(MouseButton::Left) {
        app.focus_pane(Focus::View);
        // A click on a collapsed stub expands its thread with the cursor
        // on it (ADR 0049).
        if row < text_rows
            && let Some((stub, _, _)) = app.stub_on_row(app.view().scroll() + row)
            && !stub.expanded()
        {
            let id = stub.id().clone();
            let newest = app.newest_message(&id);
            app.goto_message(id, newest);
            return Effect::None;
        }
    }
    let view = app.view_mut();
    view.touch();
    match event.kind {
        MouseEventKind::ScrollDown => view.scroll_by(WHEEL_LINES),
        MouseEventKind::ScrollUp => view.scroll_by(-WHEEL_LINES),
        MouseEventKind::Down(MouseButton::Left) if row < text_rows => view.click(row, col),
        MouseEventKind::Drag(MouseButton::Left) => {
            view.drag(row.min(text_rows.saturating_sub(1)), col);
        }
        MouseEventKind::Up(MouseButton::Left) => view.release(),
        _ => {}
    }
    Effect::None
}
