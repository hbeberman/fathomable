// @okf-doc: /decisions/0007-key-grammar-and-mouse.md
//! Translate crossterm key and mouse events into view operations.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::view::{Effect, Mode, View};

/// Lines moved per scroll-wheel notch.
const WHEEL_LINES: isize = 3;

/// Apply a key press to `view`.
pub fn handle_key(view: &mut View, key: KeyEvent) -> Effect {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        return Effect::Quit;
    }
    view.clear_message();
    match view.mode() {
        Mode::Command | Mode::Search { .. } => input_line(view, key),
        Mode::Normal | Mode::Select => normal(view, key, ctrl),
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
        (KeyCode::Char('V'), _) => view.select_lines(),
        (KeyCode::Char('y'), _) if view.mode() == Mode::Select => return view.yank(),
        (KeyCode::Esc, _) => view.escape(),
        _ => {}
    }
    Effect::None
}

/// Apply a mouse event; `gutter` is the width of the non-text columns and
/// `rows` the number of text rows.
pub fn handle_mouse(view: &mut View, event: MouseEvent, gutter: usize, rows: usize) -> Effect {
    let row = usize::from(event.row);
    let col = usize::from(event.column).saturating_sub(gutter);
    match event.kind {
        MouseEventKind::ScrollDown => view.scroll_by(WHEEL_LINES),
        MouseEventKind::ScrollUp => view.scroll_by(-WHEEL_LINES),
        MouseEventKind::Down(MouseButton::Left) if row < rows => view.click(row, col),
        MouseEventKind::Drag(MouseButton::Left) => view.drag(row.min(rows.saturating_sub(1)), col),
        MouseEventKind::Up(MouseButton::Left) if view.selection().is_some() => {
            return view.release();
        }
        _ => {}
    }
    Effect::None
}
