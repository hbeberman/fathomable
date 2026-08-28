// @okf-doc: /decisions/0007-key-grammar-and-mouse.md
//! Translate crossterm key and mouse events into app and view operations.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fathomable_core::editor::{Edit, Motion};
use fathomable_core::tree::Tree;

use super::view::{Effect, Mode, View};
use super::{App, Border, Focus, Popup, hscroll};

/// Lines moved per scroll-wheel notch.
const WHEEL_LINES: isize = 3;

/// Apply a key press to `app`. Going elsewhere switches auto-jump off
/// (ADR 0031).
pub fn handle_key(app: &mut App, key: KeyEvent) -> Effect {
    app.with_navigation_watch(|app| key_event(app, key))
}

fn key_event(app: &mut App, key: KeyEvent) -> Effect {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    app.clear_message();
    app.view_mut().clear_message();

    if app.popup().is_some() {
        return popup(app, key, ctrl);
    }
    if let Some(pending) = app.pending()
        && matches!(pending, '[' | ']')
    {
        app.set_pending(None);
        match (pending, key.code) {
            ('[', KeyCode::Char('o')) => app.history_back(),
            (']', KeyCode::Char('o')) => app.history_forward(),
            ('[', KeyCode::Char('c')) => app.prev_annotation(),
            (']', KeyCode::Char('c')) => app.next_annotation(),
            ('[', KeyCode::Char('r')) => app.waiting_prev(),
            (']', KeyCode::Char('r')) => app.waiting_next(),
            ('[', KeyCode::Char('g')) => app.hunk_prev(),
            (']', KeyCode::Char('g')) => app.hunk_next(),
            ('[', KeyCode::Char('G')) => app.dirty_prev(),
            (']', KeyCode::Char('G')) => app.dirty_next(),
            ('[', KeyCode::Char('f')) => app.jump_prev(),
            (']', KeyCode::Char('f')) => app.jump_next(),
            _ => {}
        }
        return Effect::None;
    }
    if app.focus() == Focus::View {
        // Reader activity holds auto-jump back and delays "seen" (ADR 0015).
        app.view_mut().touch();
    }
    let mode = app.view().mode();
    if matches!(mode, Mode::Command | Mode::Search { .. }) {
        return input_line(app.view_mut(), key);
    }
    if key.code == KeyCode::Char(' ') && mode == Mode::Normal {
        app.open_space_menu();
        return Effect::None;
    }
    match app.focus() {
        Focus::Thread => {
            thread(app, key);
            Effect::None
        }
        Focus::Threads => {
            thread_list(app, key, ctrl);
            Effect::None
        }
        Focus::Sidebar => {
            sidebar(app, key);
            Effect::None
        }
        Focus::FileThreads => {
            file_threads(app, key);
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
            // `c` opens the thread on the cursor row, else annotates the
            // selection or the cursor line; `C` always annotates (ADR 0027).
            KeyCode::Char('c')
                if !ctrl
                    && matches!(mode, Mode::Normal | Mode::Select)
                    && app.view().pending().is_none() =>
            {
                app.start_comment();
                Effect::None
            }
            KeyCode::Char('C')
                if !ctrl
                    && matches!(mode, Mode::Normal | Mode::Select)
                    && app.view().pending().is_none() =>
            {
                app.start_new_comment();
                Effect::None
            }
            // At column 0, `h` steps back into the tree; a selection wraps
            // instead (see `View::move_left`).
            KeyCode::Char('h') | KeyCode::Left
                if mode == Mode::Normal
                    && app.view().pending().is_none()
                    && app.tree().is_some()
                    && app.view().at_line_start() =>
            {
                app.toggle_sidebar_focus();
                Effect::None
            }
            _ => normal(app.view_mut(), key, ctrl),
        },
    }
}

fn popup(app: &mut App, key: KeyEvent, ctrl: bool) -> Effect {
    match app.popup() {
        Some(Popup::Space) => match key.code {
            KeyCode::Char(ch) => app.space_menu_select(ch),
            _ => app.close_popup(),
        },
        Some(Popup::Jump) => match key.code {
            KeyCode::Char(ch) => app.jump_menu_select(ch),
            _ => app.close_popup(),
        },
        Some(Popup::Help | Popup::Status) => app.close_popup(),
        Some(Popup::Compose(_)) => return compose(app, key, ctrl),
        Some(Popup::Picker(_)) => match (key.code, ctrl) {
            (KeyCode::Esc, _) => app.close_popup(),
            (KeyCode::Enter, _) => app.picker_confirm(),
            (KeyCode::Backspace, _) => app.picker_backspace(),
            (KeyCode::Down, _) | (KeyCode::Char('n'), true) => app.picker_move(1),
            (KeyCode::Up, _) | (KeyCode::Char('p'), true) => app.picker_move(-1),
            (KeyCode::Char(ch), false) => app.picker_char(ch),
            _ => {}
        },
        None => {}
    }
    Effect::None
}

/// Keys in the comment box (ADR 0018): readline-style motion and kills,
/// none of them a zellij lock; Alt-Up/Down and PageUp/Down scroll the
/// thread above a reply.
fn compose(app: &mut App, key: KeyEvent, ctrl: bool) -> Effect {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Esc => app.compose_cancel(),
        KeyCode::Enter if ctrl || alt => app.compose_submit(),
        KeyCode::Char('e') if ctrl => return Effect::EditDraft,
        KeyCode::Up if alt => app.compose_scroll(-1),
        KeyCode::Down if alt => app.compose_scroll(1),
        KeyCode::PageUp => app.compose_scroll(-WHEEL_LINES),
        KeyCode::PageDown => app.compose_scroll(WHEEL_LINES),
        KeyCode::Enter => app.compose_edit(Edit::Newline),
        KeyCode::Backspace => app.compose_edit(Edit::DeleteBack),
        KeyCode::Delete => app.compose_edit(Edit::DeleteForward),
        KeyCode::Left => app.compose_edit(Edit::Move(Motion::Left)),
        KeyCode::Right => app.compose_edit(Edit::Move(Motion::Right)),
        KeyCode::Up => app.compose_edit(Edit::Move(Motion::Up)),
        KeyCode::Down => app.compose_edit(Edit::Move(Motion::Down)),
        KeyCode::Home => app.compose_edit(Edit::Move(Motion::LineStart)),
        KeyCode::End => app.compose_edit(Edit::Move(Motion::LineEnd)),
        KeyCode::Char('a') if ctrl => app.compose_edit(Edit::Move(Motion::LineStart)),
        KeyCode::Char('c') if ctrl => app.compose_clear(),
        KeyCode::Char('w') if ctrl => app.compose_edit(Edit::DeleteWordBack),
        KeyCode::Char('u') if ctrl => app.compose_edit(Edit::DeleteToLineStart),
        KeyCode::Char('k') if ctrl => app.compose_edit(Edit::DeleteToLineEnd),
        KeyCode::Char('b') if alt => app.compose_edit(Edit::Move(Motion::WordBack)),
        KeyCode::Char('f') if alt => app.compose_edit(Edit::Move(Motion::WordForward)),
        KeyCode::Char(ch) if !ctrl && !alt => app.compose_insert(ch.encode_utf8(&mut [0; 4])),
        _ => {}
    }
    Effect::None
}

fn sidebar(app: &mut App, key: KeyEvent) {
    let before = highlight(app);
    sidebar_key(app, key);
    // The highlight is what the main pane shows (ADR 0023): a key that
    // moved it onto a file shows that file. Comparing paths keeps `R`, `I`,
    // and `Esc` from re-showing a highlight the reader has since left.
    if highlight(app) != before {
        app.show_highlight();
    }
}

/// Root-relative path of the row under the tree cursor.
fn highlight(app: &App) -> Option<std::path::PathBuf> {
    app.tree()
        .and_then(Tree::current)
        .map(|row| row.path().to_path_buf())
}

fn sidebar_key(app: &mut App, key: KeyEvent) {
    if let Some(pending) = app.pending() {
        app.set_pending(None);
        if pending == 'g' {
            match key.code {
                KeyCode::Char('g') => app.with_tree(|tree, _| {
                    tree.goto_top();
                    None
                }),
                KeyCode::Char('e') => app.with_tree(|tree, _| {
                    tree.goto_bottom();
                    None
                }),
                _ => {}
            }
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
        let count = view.take_count();
        match pending {
            'g' => match key.code {
                KeyCode::Char('g') => view.goto_top(),
                KeyCode::Char('e') => view.goto_bottom(),
                KeyCode::Char('s') => view.toggle_source_view(),
                KeyCode::Char('d') => view.toggle_diff_view(),
                KeyCode::Char('D') => view.toggle_seen_diff_view(),
                _ => {}
            },
            // `zl` `zh` `zL` `zH` scroll long lines sideways (ADR 0029).
            'z' => {
                hscroll::key(view, key.code, count);
            }
            _ => {}
        }
        return Effect::None;
    }
    // Digits before a key are its count (`10zl`); a leading `0` is still
    // line start.
    if let KeyCode::Char(digit) = key.code
        && !ctrl
        && let Some(value) = digit.to_digit(10)
        && (value != 0 || view.count() > 0)
    {
        view.push_count(value);
        return Effect::None;
    }
    // Only the `z` prefix carries a count on to its second key; every
    // other key drops it.
    if key.code != KeyCode::Char('z') {
        view.take_count();
    }
    match (key.code, ctrl) {
        (KeyCode::Char('z'), false) => view.set_pending(Some('z')),
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
        (KeyCode::Char('x'), _) => view.extend_line_below(),
        (KeyCode::Char('y'), _) if view.mode() == Mode::Select => return view.yank(),
        (KeyCode::Esc, _) => view.escape(),
        _ => {}
    }
    Effect::None
}

/// Keys while the thread pane has focus (ADR 0013).
fn thread(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.close_thread(),
        KeyCode::Char('j') | KeyCode::Down => app.thread_scroll(1),
        KeyCode::Char('k') | KeyCode::Up => app.thread_scroll(-1),
        KeyCode::Char('n') => app.thread_step(1),
        KeyCode::Char('p') => app.thread_step(-1),
        KeyCode::Char('r') => app.thread_reply(),
        KeyCode::Char('x') => app.thread_toggle_resolved(),
        _ => {}
    }
}

/// The mouse over the tree column: the file-threads pane along its bottom
/// (ADR 0027) takes what lands on it; the tree above pages the viewer.
fn sidebar_mouse(app: &mut App, kind: MouseEventKind, row: usize) {
    let tree_rows = app.sidebar_rows();
    if row >= tree_rows && row < app.pane_rows() {
        file_threads_mouse(app, kind, row - tree_rows);
        return;
    }
    match kind {
        // One row per tick, not `WHEEL_LINES`: each tick pages the main
        // pane to the next file (ADR 0023). The text and thread panes
        // below keep their three-line wheel.
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            let before = highlight(app);
            app.with_tree(|tree, _| {
                if kind == MouseEventKind::ScrollDown {
                    tree.move_down(1);
                } else {
                    tree.move_up(1);
                }
                None
            });
            if highlight(app) != before {
                app.show_highlight();
            }
        }
        // Row 0 is the root header.
        MouseEventKind::Down(MouseButton::Left) if row >= 1 => app.sidebar_click(row - 1),
        MouseEventKind::Down(MouseButton::Left) => app.focus_pane(Focus::Sidebar),
        _ => {}
    }
}

/// Keys in the file-threads pane (ADR 0027).
fn file_threads(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.leave_file_threads(),
        KeyCode::Char('j') | KeyCode::Down => app.file_thread_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.file_thread_move(-1),
        KeyCode::Enter => app.file_thread_open(),
        KeyCode::Char('r') => app.file_thread_reply(),
        KeyCode::Char('x') => app.file_thread_toggle_resolved(),
        KeyCode::Char(':') => app.view_mut().start_command(),
        _ => {}
    }
}

/// The mouse over the file-threads pane (ADR 0027): the wheel steps
/// between threads, a click on an entry goes to it, and the rule drags.
/// Row 0 is the rule, row 1 the header.
fn file_threads_mouse(app: &mut App, kind: MouseEventKind, row: usize) {
    match kind {
        MouseEventKind::ScrollDown => app.file_thread_move(1),
        MouseEventKind::ScrollUp => app.file_thread_move(-1),
        MouseEventKind::Down(MouseButton::Left) if row == 0 => app.begin_drag(Border::FileThreads),
        MouseEventKind::Down(MouseButton::Left) if row >= 2 => app.file_thread_click(row - 2),
        MouseEventKind::Down(MouseButton::Left) => app.focus_pane(Focus::FileThreads),
        _ => {}
    }
}

/// Keys in the thread list (ADR 0025).
fn thread_list(app: &mut App, key: KeyEvent, ctrl: bool) {
    if let Some(pending) = app.pending() {
        app.set_pending(None);
        if pending == 'g' {
            match key.code {
                KeyCode::Char('g') => app.thread_list_goto(false),
                KeyCode::Char('e') => app.thread_list_goto(true),
                _ => {}
            }
        }
        return;
    }
    let half = isize::try_from(app.text_rows() / 2)
        .unwrap_or(isize::MAX)
        .max(1);
    match (key.code, ctrl) {
        (KeyCode::Esc, _) => app.close_thread_list(),
        (KeyCode::Char('j') | KeyCode::Down, _) => app.thread_list_move(1),
        (KeyCode::Char('k') | KeyCode::Up, _) => app.thread_list_move(-1),
        (KeyCode::Char('d'), true) | (KeyCode::PageDown, _) => app.thread_list_move(half),
        (KeyCode::Char('u'), true) | (KeyCode::PageUp, _) => app.thread_list_move(-half),
        (KeyCode::Char('g'), _) => app.set_pending(Some('g')),
        (KeyCode::Char('G'), _) => app.thread_list_goto(true),
        (KeyCode::Enter, _) => app.thread_list_open_entry(),
        (KeyCode::Char('r'), _) => app.thread_list_reply(),
        (KeyCode::Char('x'), _) => app.thread_list_toggle_resolved(),
        (KeyCode::Char('z'), _) => app.thread_list_fold(),
        (KeyCode::Char('Z'), _) => app.thread_list_fold_resolved(),
        (KeyCode::Char('f'), _) => app.thread_list_toggle_file(),
        (KeyCode::Char(':'), _) => app.view_mut().start_command(),
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

/// Apply a mouse event to whichever pane it lands on: the wheel scrolls
/// the pane under the pointer, a click focuses it, and a press on the
/// tree's divider or the thread pane's rule drags that border.
pub fn handle_mouse(app: &mut App, event: MouseEvent) -> Effect {
    app.with_navigation_watch(|app| mouse_event(app, event))
}

fn mouse_event(app: &mut App, event: MouseEvent) -> Effect {
    // The comment box keeps the keys but not the mouse: the reader can
    // scroll, click, and resize around it while writing.
    if matches!(
        app.popup(),
        Some(Popup::Space | Popup::Jump | Popup::Help | Popup::Status | Popup::Picker(_))
    ) {
        return Effect::None;
    }
    let row = usize::from(event.row);
    let column = usize::from(event.column);
    let rows = app.pane_rows();
    let sidebar = app.sidebar_width();
    // The box sits under the thread pane and pushes it up (ADR 0013).
    let box_rows = app.compose_rows();
    let box_top = rows.saturating_sub(box_rows);
    let thread_top = box_top.saturating_sub(app.thread_rows());
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
        if app.thread_rows() > 0 && column >= sidebar && row == thread_top {
            app.begin_drag(Border::Thread);
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
    if row >= thread_top && row < box_top {
        match event.kind {
            MouseEventKind::ScrollDown => app.thread_scroll(WHEEL_LINES),
            MouseEventKind::ScrollUp => app.thread_scroll(-WHEEL_LINES),
            MouseEventKind::Down(MouseButton::Left) => app.focus_pane(Focus::Thread),
            _ => {}
        }
        return Effect::None;
    }
    if app.thread_list().is_open() {
        thread_list_mouse(app, event.kind, row);
        return Effect::None;
    }
    let gutter = sidebar + super::ui::gutter_width(app.view());
    let col = column.saturating_sub(gutter);
    let text_rows = app.text_rows();
    if event.kind == MouseEventKind::Down(MouseButton::Left) {
        app.focus_pane(Focus::View);
    }
    let view = app.view_mut();
    view.touch();
    if hscroll::mouse(view, event.kind) {
        return Effect::None;
    }
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
