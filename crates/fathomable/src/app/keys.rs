// @okf-doc: /decisions/0007-key-grammar-and-mouse.md
//! Translate crossterm key and mouse events into app and view operations.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fathomable_core::tree::Tree;

use super::view::{Effect, Mode, View};
use super::{App, Focus, Popup};

/// Lines moved per scroll-wheel notch.
const WHEEL_LINES: isize = 3;

/// Apply a key press to `app`.
pub fn handle_key(app: &mut App, key: KeyEvent) -> Effect {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        return Effect::Quit;
    }
    app.clear_message();
    app.view_mut().clear_message();

    if app.popup().is_some() {
        popup(app, key, ctrl);
        return Effect::None;
    }
    if let Some(pending) = app.pending()
        && matches!(pending, '[' | ']')
    {
        app.set_pending(None);
        match (pending, key.code) {
            ('[', KeyCode::Char('o')) => app.history_back(),
            (']', KeyCode::Char('o')) => app.history_forward(),
            ('[', KeyCode::Char('a')) => app.prev_annotation(),
            (']', KeyCode::Char('a')) => app.next_annotation(),
            ('[', KeyCode::Char('c')) => app.view_mut().prev_hunk(),
            (']', KeyCode::Char('c')) => app.view_mut().next_hunk(),
            _ => {}
        }
        return Effect::None;
    }
    let mode = app.view().mode();
    if matches!(mode, Mode::Command | Mode::Search { .. }) {
        return input_line(app.view_mut(), key);
    }
    if ctrl && key.code == KeyCode::Char('b') {
        app.toggle_sidebar_focus();
        return Effect::None;
    }
    if key.code == KeyCode::Char(' ') && mode == Mode::Normal {
        app.open_space_menu();
        return Effect::None;
    }
    match app.focus() {
        Focus::Sidebar => {
            sidebar(app, key);
            Effect::None
        }
        Focus::View => match key.code {
            KeyCode::Char('[') if mode == Mode::Normal => {
                app.set_pending(Some('['));
                Effect::None
            }
            KeyCode::Char(']') if mode == Mode::Normal => {
                app.set_pending(Some(']'));
                Effect::None
            }
            KeyCode::Char('c') if mode == Mode::Select => {
                app.start_comment();
                Effect::None
            }
            _ => normal(app.view_mut(), key, ctrl),
        },
    }
}

fn popup(app: &mut App, key: KeyEvent, ctrl: bool) {
    match app.popup() {
        Some(Popup::Space) => match key.code {
            KeyCode::Char(ch) => app.space_menu_select(ch),
            _ => app.close_popup(),
        },
        Some(Popup::Help) => app.close_popup(),
        Some(Popup::Compose(_)) => {
            let submit = key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
            match key.code {
                KeyCode::Esc => app.compose_cancel(),
                KeyCode::Enter if submit => app.compose_submit(),
                KeyCode::Enter => app.compose_newline(),
                KeyCode::Backspace => app.compose_backspace(),
                KeyCode::Down => app.compose_scroll(1),
                KeyCode::Up => app.compose_scroll(-1),
                KeyCode::Char(ch) if !ctrl => app.compose_char(ch),
                _ => {}
            }
        }
        Some(Popup::Thread(_)) => match key.code {
            KeyCode::Esc => app.close_popup(),
            KeyCode::Char('j') | KeyCode::Down => app.thread_scroll(1),
            KeyCode::Char('k') | KeyCode::Up => app.thread_scroll(-1),
            KeyCode::Char('n') => app.thread_step(1),
            KeyCode::Char('p') => app.thread_step(-1),
            KeyCode::Char('r') => app.thread_reply(),
            KeyCode::Char('x') => app.thread_toggle_resolved(),
            _ => {}
        },
        Some(Popup::Picker(_)) => match (key.code, ctrl) {
            (KeyCode::Esc, _) => app.close_popup(),
            (KeyCode::Enter, _) => app.picker_confirm(),
            (KeyCode::Backspace, _) => app.picker_backspace(),
            (KeyCode::Down, _) | (KeyCode::Char('j' | 'n'), true) => app.picker_move(1),
            (KeyCode::Up, _) | (KeyCode::Char('k' | 'p'), true) => app.picker_move(-1),
            (KeyCode::Char(ch), false) => app.picker_char(ch),
            _ => {}
        },
        None => {}
    }
}

fn sidebar(app: &mut App, key: KeyEvent) {
    if let Some(pending) = app.pending() {
        app.set_pending(None);
        if pending == 'g' && key.code == KeyCode::Char('g') {
            app.with_tree(|tree, _| {
                tree.goto_top();
                None
            });
        }
        return;
    }
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.with_tree(|tree, _| {
            tree.move_down(1);
            None
        }),
        KeyCode::Char('k') | KeyCode::Up => app.with_tree(|tree, _| {
            tree.move_up(1);
            None
        }),
        KeyCode::Char('h') | KeyCode::Left => app.with_tree(|tree, _| {
            tree.collapse();
            None
        }),
        KeyCode::Char('l') | KeyCode::Right => {
            app.with_tree_result(Tree::expand);
        }
        KeyCode::Enter => app.with_tree_result(Tree::activate),
        KeyCode::Char('g') => app.set_pending(Some('g')),
        KeyCode::Char('G') => app.with_tree(|tree, _| {
            tree.goto_bottom();
            None
        }),
        KeyCode::Char('R') => app.refresh_tree(),
        KeyCode::Char('I') => app.toggle_ignored(),
        KeyCode::Char(':') => {
            app.toggle_sidebar_focus();
            app.view_mut().start_command();
        }
        KeyCode::Esc => app.toggle_sidebar_focus(),
        _ => {}
    }
}

fn input_line(view: &mut View, key: KeyEvent) -> Effect {
    match key.code {
        KeyCode::Esc => view.escape(),
        KeyCode::Enter => return view.confirm(),
        KeyCode::Backspace => view.input_backspace(),
        KeyCode::Char(ch) => view.input_char(ch),
        _ => {}
    }
    Effect::None
}

fn normal(view: &mut View, key: KeyEvent, ctrl: bool) -> Effect {
    if let Some(pending) = view.pending() {
        view.set_pending(None);
        if pending == 'g' {
            match key.code {
                KeyCode::Char('g') => view.goto_top(),
                KeyCode::Char('s') => view.toggle_source_view(),
                KeyCode::Char('d') => view.toggle_diff_view(),
                _ => {}
            }
        }
        return Effect::None;
    }
    match (key.code, ctrl) {
        (KeyCode::Char('d'), true) => view.half_page_down(),
        (KeyCode::Char('u'), true) => view.half_page_up(),
        (KeyCode::Char('j') | KeyCode::Down, _) => view.move_down(1),
        (KeyCode::Char('k') | KeyCode::Up, _) => view.move_up(1),
        (KeyCode::Char('h') | KeyCode::Left, _) => view.move_left(),
        (KeyCode::Char('l') | KeyCode::Right, _) => view.move_right(),
        (KeyCode::Char('0') | KeyCode::Home, _) => view.line_start(),
        (KeyCode::Char('$') | KeyCode::End, _) => view.line_end(),
        (KeyCode::Char('g'), _) => view.set_pending(Some('g')),
        (KeyCode::Char('G'), _) => view.goto_bottom(),
        (KeyCode::Char('/'), _) => view.start_search(false),
        (KeyCode::Char('?'), _) => view.start_search(true),
        (KeyCode::Char('n'), _) => view.search_next(false),
        (KeyCode::Char('N'), _) => view.search_next(true),
        (KeyCode::Char(':'), _) => view.start_command(),
        (KeyCode::Char('v'), _) => view.select_chars(),
        (KeyCode::Char('V'), _) => view.select_lines(),
        (KeyCode::Char('y'), _) if view.mode() == Mode::Select => return view.yank(),
        (KeyCode::Esc, _) => view.escape(),
        _ => {}
    }
    Effect::None
}

/// Apply a mouse event to whichever pane it lands on.
pub fn handle_mouse(app: &mut App, event: MouseEvent) -> Effect {
    if app.popup().is_some() {
        return Effect::None;
    }
    let row = usize::from(event.row);
    let column = usize::from(event.column);
    let rows = app.pane_rows();
    let sidebar = app.sidebar_width();
    if column < sidebar {
        match event.kind {
            MouseEventKind::ScrollDown => app.with_tree(|tree, _| {
                tree.move_down(WHEEL_LINES.unsigned_abs());
                None
            }),
            MouseEventKind::ScrollUp => app.with_tree(|tree, _| {
                tree.move_up(WHEEL_LINES.unsigned_abs());
                None
            }),
            // Row 0 is the root header.
            MouseEventKind::Down(MouseButton::Left) if row >= 1 && row < rows => {
                app.sidebar_click(row - 1);
            }
            _ => {}
        }
        return Effect::None;
    }
    let gutter = sidebar + super::ui::gutter_width(app.view());
    let col = column.saturating_sub(gutter);
    let view = app.view_mut();
    match event.kind {
        MouseEventKind::ScrollDown => view.scroll_by(WHEEL_LINES),
        MouseEventKind::ScrollUp => view.scroll_by(-WHEEL_LINES),
        MouseEventKind::Down(MouseButton::Left) if row < rows => view.click(row, col),
        MouseEventKind::Drag(MouseButton::Left) => view.drag(row.min(rows.saturating_sub(1)), col),
        MouseEventKind::Up(MouseButton::Left) => view.release(),
        _ => {}
    }
    Effect::None
}
