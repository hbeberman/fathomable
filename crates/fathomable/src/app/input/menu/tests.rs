use std::fs;

use anyhow::Context as _;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fathomable_core::tree::Tree;
use fathomable_testing::TempDir;

use crate::app::testing::{self, app, key};

use super::super::bindings::{self, Action, Where};
use super::super::keys::handle_key;
use super::super::mouse::handle_mouse;
use super::Menu;
use crate::app::draw;
use crate::app::draw::header;
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
    let gutter = app.sidebar_width() + draw::gutter_width(app.view());
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
    let sidebar = app.sidebar_width();
    left(&mut app, sidebar, screen_row);
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
    left(&mut app, sidebar, screen_row);
    right(&mut app, column, screen_row);
    handle_key(&mut app, key('c'));
    assert!(
        matches!(app.popup(), Some(Popup::Compose(_))),
        "c comments on the selection"
    );
    Ok(())
}

#[test]
fn right_click_outside_the_selection_moves_the_cursor_and_offers_the_line() -> anyhow::Result<()> {
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
    let sidebar = app.sidebar_width();
    let (_, alpha_row) = at(&app, alpha, 0);
    let (_, gamma_row) = at(&app, gamma, 0);
    left(&mut app, sidebar, alpha_row);
    assert_eq!(app.view().selected_source().as_deref(), Some("alpha beta"));
    mouse(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        sidebar,
        gamma_row,
    );
    mouse(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        sidebar,
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

/// ADR 0073: a stub draws `▸`, and a click on it or a double-click on
/// the stub expands the thread; the expanded header draws `▾` in the
/// thread's gutter, a click there folds the thread, and so does a
/// double-click anywhere on the header. One click on either row
/// places the cursor. A double-click on a stub expands it and stops.
#[test]
fn the_chevron_and_a_double_click_fold_and_unfold_the_thread() -> anyhow::Result<()> {
    let dir = fixture("chevron")?;
    let mut app = app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    app.compose_insert("first");
    app.compose_submit();
    let id = app
        .file_threads()
        .into_iter()
        .next()
        .context("the thread")?;
    app.fold_thread(&id);
    let gutter = app.sidebar_width() + draw::gutter_width(app.view());
    let after = |row: &str| row.chars().skip(gutter).collect::<String>();
    let top = (0..app.view().layout().lines().len())
        .find(|&row| app.stub_on_row(row).is_some())
        .context("the stub's first row")?;
    let (_, screen_row) = at(&app, top, 0);
    let rows = testing::screen(&app)?;
    // The cursor is on the thread's line, so its stub carries the bar
    // in the edge cell (ADR 0071, amended 2026-09-09).
    assert!(
        rows.iter().any(|row| after(row).starts_with("▎▸ ●")),
        "the stub draws the chevron: {rows:?}"
    );

    // One click on the stub's words places the cursor; two expand it
    // and stop.
    let (words, _) = at(&app, top, 12);
    left(&mut app, words, screen_row);
    assert!(
        !app.is_expanded(&id),
        "one click on the stub keeps it folded"
    );
    assert_eq!(app.view().mode(), Mode::Normal, "and selects nothing");
    left(&mut app, words, screen_row);
    assert!(
        app.is_expanded(&id),
        "a double-click on the stub expands it"
    );
    left(&mut app, words, screen_row);
    assert!(
        app.is_expanded(&id),
        "the third press is a first press on the header"
    );
    let rows = testing::screen(&app)?;
    assert!(
        rows.iter().any(|row| after(row).starts_with("▎▾ ●")),
        "the header draws the chevron after the bar: {rows:?}"
    );

    // A click on the chevron folds.
    let (chevron, _) = at(&app, top, 1);
    left(&mut app, chevron, screen_row);
    assert!(!app.is_expanded(&id), "a click on the chevron folds");

    // A click on the stub's chevron expands it.
    left(&mut app, chevron, screen_row);
    assert!(
        app.is_expanded(&id),
        "a click on the stub's chevron expands"
    );

    // One click on the header's words places the cursor; two fold.
    left(&mut app, words, screen_row);
    assert!(
        app.is_expanded(&id),
        "one click on the header keeps it open"
    );
    assert_eq!(app.view().mode(), Mode::Normal, "and selects nothing");
    left(&mut app, words, screen_row);
    assert!(!app.is_expanded(&id), "a double-click on the header folds");
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
            "only changed",
            "hide untracked",
            "show ignored"
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

    // The text's key bar (ADR 0067): the expanded thread's `reply` hint.
    let row = row_of(&app, "alpha beta")?;
    app.view_mut().goto_row(row);
    handle_key(&mut app, key('c'));
    let width = app.column_width();
    let sidebar = app.sidebar_width();
    let bar_row = app.text_bar_row();
    let col = (0..width)
        .find(|&c| draw::bar::text_bar(&app).action_at(width, c) == Some(Action::Reply))
        .context("reply is drawn")?;
    left(&mut app, sidebar + col, bar_row);
    assert!(matches!(
        app.popup(),
        Some(Popup::Compose(compose)) if matches!(compose.target(), ComposeTarget::Reply(_))
    ));
    // The draft's keys on the bar (ADR 0054, ADR 0067): `Esc` cancels.
    let col = (0..width)
        .find(|&c| draw::bar::text_bar(&app).action_at(width, c) == Some(Action::Escape))
        .context("Esc is drawn")?;
    left(&mut app, sidebar + col, bar_row);
    assert!(app.popup().is_none(), "the draft closed");

    // The review list's key bar: the `resolved` hint toggles the flag
    // (ADR 0059).
    app.toggle_review();
    assert_eq!(app.focus(), Focus::Review);
    let list_rows = app.review_rows(app.column_width());
    let bar = header::review_footer(&app, &list_rows.entries);
    let col = (0..width)
        .find(|&c| bar.action_at(width, c) == Some(Action::ReviewResolved))
        .context("resolved is drawn on the bar")?;
    let before = app.review().resolved;
    let bar_row = app.text_bar_row();
    left(&mut app, sidebar + col, bar_row);
    assert_ne!(app.review().resolved, before, "the resolved hint ran");

    // The header's resolved count, once there is one, toggles it back
    // (ADR 0066).
    app.thread_toggle_resolved();
    let header = header::review_header(&app);
    let col = (0..width)
        .find(|&c| header.action_at(width, c) == Some(Action::ReviewResolved))
        .context("the resolved count is drawn")?;
    left(&mut app, sidebar + col, 0);
    assert_eq!(app.review().resolved, before, "the count ran x");
    Ok(())
}

/// The status line's counts take clicks (ADR 0066): the waiting count
/// opens the review list and the thread count focuses the threads pane.
#[test]
fn the_status_line_counts_take_clicks() -> anyhow::Result<()> {
    use fathomable_core::annotations::Author;

    let dir = fixture("status")?;
    let mut app = app(&dir)?;
    annotate(&mut app)?;
    let id = app.file_threads()[0].clone();
    app.agent_reply(
        &id,
        Author::agent("reviewer"),
        "done".to_owned(),
        false,
        None,
    )
    .map_err(anyhow::Error::msg)?;
    let parts = draw::status_parts(&app);
    let text = parts.right_text();
    assert!(text.contains("● 1 waiting"), "{text}");
    let start = app.size().0 - parts.right_width();
    let waiting = text.find("1 waiting").context("the count")?;
    let status_row = app.pane_rows();
    left(&mut app, start + waiting, status_row);
    assert!(
        app.review_list().is_open(),
        "the waiting count opens the review"
    );
    app.close_review();
    let threads = text.find("1 threads").context("the count")?;
    left(&mut app, start + threads, status_row);
    assert_eq!(app.focus(), Focus::ThreadsPane);
    Ok(())
}

/// In the review list a click on a thread's chevron or a double-click
/// on its row folds and expands it, the thread's menu offers `fold
/// thread` and no `fold file`, a click on a file row folds the file and
/// rests the cursor on it, the file's menu has no fold-all, and a file
/// row draws `▾` open and `▸` folded (ADR 0076).
#[test]
fn the_list_folds_a_thread_by_chevron_double_click_and_menu() -> anyhow::Result<()> {
    let dir = fixture("list-fold")?;
    let mut app = app(&dir)?;
    annotate(&mut app)?;
    let id = app.file_threads()[0].clone();
    app.open_review();
    let edge = app.sidebar_width();
    let after = |row: &str| row.chars().skip(edge).collect::<String>();
    let rows = testing::screen(&app)?;
    let file_y = rows
        .iter()
        .position(|row| row.contains("▾ README.md"))
        .with_context(|| format!("the file row with its arrow: {rows:?}"))?;
    // The header sits in the nest (ADR 0077): the bar, two cells, the
    // chevron under the path's first letter.
    let header_y = rows
        .iter()
        .position(|row| after(row).starts_with("▎  ▾ ●"))
        .with_context(|| format!("the header with its chevron: {rows:?}"))?;

    // The chevron cell folds and expands.
    left(&mut app, edge + 3, header_y);
    assert!(app.review_list().is_thread_folded(&id), "the chevron folds");
    let rows = testing::screen(&app)?;
    assert!(
        after(&rows[header_y]).starts_with("▎  ▸ ●"),
        "the folded row draws `▸`: {rows:?}"
    );
    left(&mut app, edge + 3, header_y);
    assert!(
        !app.review_list().is_thread_folded(&id),
        "the chevron expands"
    );

    // One click on the words selects; two fold and end the gesture.
    left(&mut app, edge + 12, header_y);
    assert!(
        !app.review_list().is_thread_folded(&id),
        "one click selects"
    );
    left(&mut app, edge + 12, header_y);
    assert!(
        app.review_list().is_thread_folded(&id),
        "a double-click folds"
    );
    left(&mut app, edge + 12, header_y);
    assert!(
        app.review_list().is_thread_folded(&id),
        "the third press is a first press"
    );

    // The thread's menu leads with the fold and has no `fold file`.
    right(&mut app, edge + 12, header_y);
    let labels: Vec<String> = entries(&app)?.into_iter().map(|(_, label)| label).collect();
    assert_eq!(labels[0], "expand thread");
    assert!(
        !labels.iter().any(|label| label == "fold file"),
        "{labels:?}"
    );
    let cell = entry_cell(&app, "expand thread")?;
    left(&mut app, cell.0, cell.1);
    assert!(
        !app.review_list().is_thread_folded(&id),
        "the entry expands"
    );

    // A click on the file row folds the file and rests the cursor on it;
    // its menu has no fold-all.
    left(&mut app, edge + 5, file_y);
    assert!(
        app.review_list()
            .is_folded(std::path::Path::new("README.md"))
    );
    let rows = testing::screen(&app)?;
    assert!(
        after(&rows[file_y]).starts_with("▎▸ README.md"),
        "folded, barred: {rows:?}"
    );
    right(&mut app, edge + 5, file_y);
    let labels: Vec<String> = entries(&app)?.into_iter().map(|(_, label)| label).collect();
    assert_eq!(labels, ["unfold", "open file", "show resolved"]);
    app.close_popup();
    Ok(())
}

/// A right-click on a file row offers the file's menu, and its `fold`
/// entry folds the file on the surface it opened for (ADR 0066); the
/// files pane's menu offers `threads` and `review` on a file with
/// threads.
#[test]
fn file_rows_and_the_files_pane_open_their_menus() -> anyhow::Result<()> {
    let dir = fixture("file-menu")?;
    let mut app = app(&dir)?;
    annotate(&mut app)?;
    app.show_tree();
    if app.threads_pane_height() == 0 {
        app.toggle_threads_pane_shown();
    }
    app.threads_pane_toggle_scope();
    let top = app.tree_rows();
    // The first body row is README's file row.
    right(&mut app, 2, top + 2);
    let labels: Vec<String> = entries(&app)?.into_iter().map(|(_, label)| label).collect();
    assert_eq!(labels, ["fold", "fold all", "open file", "show resolved"]);
    let cell = entry_cell(&app, "fold")?;
    left(&mut app, cell.0, cell.1);
    assert!(app.threads_pane_is_folded(std::path::Path::new("README.md")));
    assert!(
        !app.review_list()
            .is_folded(std::path::Path::new("README.md")),
        "the list keeps its own folds"
    );
    right(&mut app, 2, top + 2);
    let labels: Vec<String> = entries(&app)?.into_iter().map(|(_, label)| label).collect();
    assert_eq!(labels[..2], ["unfold", "unfold all"]);
    app.close_popup();

    // The files pane: `threads` puts the pane in file scope on the file.
    let readme_row = (0..app.tree_rows())
        .find(|&row| {
            app.tree()
                .and_then(|tree| tree.rows().get(row))
                .is_some_and(|r| r.name() == "README.md")
        })
        .context("README in the tree")?;
    right(&mut app, 2, readme_row + 1);
    let labels: Vec<String> = entries(&app)?.into_iter().map(|(_, label)| label).collect();
    assert!(labels.contains(&"threads".to_owned()), "{labels:?}");
    assert!(labels.contains(&"review".to_owned()), "{labels:?}");
    let cell = entry_cell(&app, "threads")?;
    left(&mut app, cell.0, cell.1);
    assert_eq!(app.focus(), Focus::ThreadsPane);
    assert_eq!(
        app.sidebar_scope(),
        crate::app::threads::pane::PaneScope::File
    );
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
    let before = app.sidebar_scope();
    let header_row = app.tree_rows() + 1;
    left(&mut app, 1, header_row);
    assert_ne!(app.sidebar_scope(), before);
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
