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
    assert_eq!(keys, ["c", "Sp c c", "y", "Esc"]);

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
            "enable auto-resolve",
            "resolve thread",
            "edit message",
            "delete thread",
            "comment on line",
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
    let reply = entries(&app)?
        .into_iter()
        .find(|(_, label)| label == "reply");
    assert_eq!(reply.map(|(key, _)| key).as_deref(), Some("Sp c r"));

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
        rows.iter().any(|row| after(row).starts_with("▎● ▸")),
        "the stub draws the chevron: {rows:?}"
    );

    // One click on the stub's words places the cursor; two expand it
    // and stop.
    let (words, _) = at(&app, top, 70);
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
        rows.iter().any(|row| after(row).starts_with("▎● ▾")),
        "the header draws the chevron after the bar: {rows:?}"
    );

    // A click on the chevron folds.
    let (chevron, _) = at(&app, top, 3);
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
fn header_actions_use_exact_cells_and_the_rows_explicit_thread() -> anyhow::Result<()> {
    let dir = fixture("header-actions")?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app)?;
    let first = app.file_threads()[0].clone();
    app.view_mut().goto_source_line(5);
    app.start_new_comment();
    app.compose_insert("second");
    app.compose_submit();
    let second = app.file_threads()[1].clone();
    app.expand_thread(first.clone());
    app.goto_message(second.clone(), 0);

    let stubs = app.stubs();
    let block = stubs
        .iter()
        .position(|stub| stub.thread() == Some(&first))
        .context("first thread block")?;
    let view_row = app
        .view()
        .row_of_stub_slot(block, 0)
        .context("first header row")?;
    let screen_row = app.text_top() + view_row - app.view().scroll();
    let gutter = draw::gutter_width(app.view());
    let origin = app.sidebar_width() + gutter;
    let width = app.column_width().saturating_sub(gutter);
    let thread = app.thread(&first).context("first thread")?;
    let layout = header::expanded_header(&app, thread, false, true, width);

    let auto = (0..width)
        .find(|column| layout.action_at(*column) == Some(Action::ToggleAutoResolve))
        .context("auto-resolve action")?;
    left(&mut app, origin + auto, screen_row);
    assert!(
        app.thread(&first)
            .context("first thread")?
            .auto_resolve()
            .is_enabled()
    );
    assert!(
        !app.thread(&second)
            .context("second thread")?
            .auto_resolve()
            .is_enabled(),
        "the non-cursor row action targets its own thread"
    );
    left(&mut app, origin + auto, screen_row);
    assert!(
        app.is_expanded(&first),
        "action clicks take precedence over double-click folding"
    );
    left(&mut app, origin + auto, screen_row);

    let thread = app.thread(&first).context("first thread")?;
    let enabled_layout = header::expanded_header(&app, thread, false, true, width);
    let enabled_auto = (0..width)
        .find(|column| enabled_layout.action_at(*column) == Some(Action::ToggleAutoResolve))
        .context("enabled auto-resolve action")?;
    let first_action_end = (enabled_auto..width)
        .find(|column| enabled_layout.action_at(*column) != Some(Action::ToggleAutoResolve))
        .context("separator after auto-resolve")?;
    left(&mut app, origin + first_action_end, screen_row);
    assert!(
        app.thread(&first)
            .context("first thread")?
            .auto_resolve()
            .is_enabled(),
        "the separator is inert"
    );

    let padded_disclosure = (0..width)
        .rev()
        .find(|column| layout.disclosure_at(*column))
        .context("disclosure padding")?;
    left(&mut app, origin + padded_disclosure, screen_row);
    assert!(!app.is_expanded(&first), "the disclosure padding folds");
    Ok(())
}

#[test]
fn a_collapsed_header_has_no_invisible_actions_away_from_the_thread() -> anyhow::Result<()> {
    let dir = fixture("collapsed-header-actions")?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app)?;
    let id = app.file_threads()[0].clone();
    if app.is_expanded(&id) {
        app.fold_thread(&id);
    }
    handle_key(&mut app, key('G'));
    assert_eq!(app.thread_cursor().thread(), Some(&id));
    assert!(!app.threads_at_cursor().contains(&id));

    let stubs = app.stubs();
    let block = stubs
        .iter()
        .position(|stub| stub.thread() == Some(&id))
        .context("thread block")?;
    let view_row = app
        .view()
        .row_of_stub_slot(block, 0)
        .context("collapsed header row")?;
    let screen_row = app.text_top() + view_row - app.view().scroll();
    let gutter = draw::gutter_width(app.view());
    let origin = app.sidebar_width() + gutter;
    let width = app.column_width().saturating_sub(gutter);
    let thread = app.thread(&id).context("thread")?;
    let hypothetical = header::expanded_header(&app, thread, true, false, width);
    let hidden_action = (0..width)
        .find(|column| hypothetical.action_at(*column) == Some(Action::ToggleAutoResolve))
        .context("action shown while the thread is selected")?;

    left(&mut app, origin + hidden_action, screen_row);
    assert!(
        !app.thread(&id)
            .context("thread")?
            .auto_resolve()
            .is_enabled(),
        "clicking preview text cannot run a hidden action"
    );
    Ok(())
}

#[test]
fn header_hover_patches_only_the_visible_action_cells() -> anyhow::Result<()> {
    use ratatui::style::{Color, Style};

    let dir = fixture("header-hover")?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app)?;
    let id = app.file_threads()[0].clone();
    let stubs = app.stubs();
    let block = stubs
        .iter()
        .position(|stub| stub.thread() == Some(&id))
        .context("thread block")?;
    let view_row = app
        .view()
        .row_of_stub_slot(block, 0)
        .context("header row")?;
    let screen_row = app.text_top() + view_row - app.view().scroll();
    let gutter = draw::gutter_width(app.view());
    let origin = app.sidebar_width() + gutter;
    let width = app.column_width().saturating_sub(gutter);
    let thread = app.thread(&id).context("thread")?;
    let layout = header::expanded_header(&app, thread, true, false, width);
    let action = (0..width)
        .find(|column| layout.action_at(*column) == Some(Action::ToggleAutoResolve))
        .context("visible action")?;
    mouse(&mut app, MouseEventKind::Moved, origin + action, screen_row);

    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let mut theme = draw::Theme::from_core(&core);
    let hover = Color::Rgb(1, 2, 3);
    theme.list_hover = Style::default().bg(hover);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
    terminal.draw(|frame| draw::draw(frame, &app, &theme))?;
    let buffer = terminal.backend().buffer();
    assert_eq!(
        buffer[(u16::try_from(origin + action)?, u16::try_from(screen_row)?)].bg,
        hover
    );
    assert_ne!(
        buffer[(
            u16::try_from(origin + action.saturating_sub(1))?,
            u16::try_from(screen_row)?,
        )]
            .bg,
        hover,
        "hover does not bleed into disclosure padding"
    );
    Ok(())
}

#[test]
fn review_header_action_targets_a_non_cursor_thread() -> anyhow::Result<()> {
    use fathomable_core::annotations::Lifecycle;

    let dir = fixture("review-header-action")?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app)?;
    let first = app.file_threads()[0].clone();
    app.view_mut().goto_source_line(5);
    app.start_new_comment();
    app.compose_insert("second");
    app.compose_submit();
    let second = app.file_threads()[1].clone();
    app.goto_message(second.clone(), 0);
    app.open_review();

    let width = app.column_width();
    let rows = app.review_rows(width);
    let (model_row, summary, selected) = rows
        .rows
        .iter()
        .enumerate()
        .find_map(|(row, item)| match item {
            crate::app::threads::list::Row::Header {
                summary, selected, ..
            } if summary.id() == &first => Some((row, summary, *selected)),
            _ => None,
        })
        .context("first review header")?;
    assert!(!selected);
    let layout = header::entry_header(summary, fathomable_core::clock::now(), false, true, width);
    let action = (0..width)
        .find(|column| layout.action_at(*column) == Some(Action::ToggleResolved))
        .context("resolve action")?;
    let screen_row = app.pane_top() + 1 + model_row - app.review_list().scroll();
    let column = app.sidebar_width() + action;
    left(&mut app, column, screen_row);
    assert_eq!(
        app.thread(&first).context("first")?.lifecycle(),
        Lifecycle::Resolved
    );
    assert_eq!(
        app.thread(&second).context("second")?.lifecycle(),
        Lifecycle::Active
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
        ["open", "file comment", "copy path", "copy full path"]
    );
    let (x, y) = entry_cell(&app, "copy path")?;
    assert_eq!(left(&mut app, x, y), Effect::Copy("README.md".to_owned()));
    assert_eq!(
        handle_key(&mut app, key('y')),
        Effect::Copy("README.md".to_owned())
    );
    right(&mut app, 0, index + 1);
    let full = testing::root(&dir)
        .join("README.md")
        .to_string_lossy()
        .into_owned();
    let (x, y) = entry_cell(&app, "copy full path")?;
    assert_eq!(left(&mut app, x, y), Effect::Copy(full.clone()));
    assert_eq!(handle_key(&mut app, key('Y')), Effect::Copy(full));
    Ok(())
}

#[test]
fn links_open_from_one_key_and_menu_action() -> anyhow::Result<()> {
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
    assert_eq!(handle_key(&mut app, key('y')), Effect::None);
    handle_key(&mut app, key('g'));
    assert_eq!(handle_key(&mut app, key('x')), Effect::None);
    handle_key(&mut app, key('g'));
    assert_eq!(
        handle_key(&mut app, key('f')),
        Effect::Open(LINK.to_owned())
    );

    right(&mut app, column, screen_row);
    let keys: Vec<(String, String)> = entries(&app)?;
    assert!(keys.contains(&("gf".to_owned(), "open linked file/URL".to_owned())));
    assert!(!keys.iter().any(|(key, _)| key == "gy" || key == "gx"));
    // A two-key entry waits for its second key.
    assert_eq!(handle_key(&mut app, key('g')), Effect::None);
    assert!(app.menu().is_some());
    assert_eq!(
        handle_key(&mut app, key('f')),
        Effect::Open(LINK.to_owned())
    );
    assert!(app.menu().is_none());
    assert_eq!(
        handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: u16::try_from(column)?,
                row: u16::try_from(screen_row)?,
                modifiers: KeyModifiers::CONTROL,
            },
        ),
        Effect::Open(LINK.to_owned())
    );
    Ok(())
}

#[test]
fn chord_helpers_anchor_to_the_viewer_bottom_right_from_every_pane() -> anyhow::Result<()> {
    let dir = fixture("menu-corner")?;
    let mut app = app(&dir)?;
    for (width, height) in [(120, 30), (60, 12), (25, 5), (10, 2), (1, 1)] {
        app.resize(width, height);
        for focus in [Focus::View, Focus::Tree, Focus::ThreadsPane, Focus::Review] {
            app.focus = focus;
            for prefix in ["g", " ", " v", " d"] {
                app.take_prefix();
                for ch in prefix.chars() {
                    handle_key(&mut app, key(ch));
                }

                let place = super::super::keys::place(&app).context("pane receives keys")?;
                let shown = bindings::menu(place, app.prefix());
                let grid = draw::which_key_grid(&app, &shown);
                assert_eq!(grid.x + grid.width, width);
                assert_eq!(grid.y + grid.height, app.pane_top() + app.pane_rows());
                assert!(grid.height <= app.pane_rows());
            }
        }
    }
    Ok(())
}

#[test]
fn space_helper_uses_a_rounded_breadcrumb_border() -> anyhow::Result<()> {
    let dir = fixture("space-border")?;
    let mut app = app(&dir)?;
    handle_key(&mut app, key(' '));
    let place = super::super::keys::place(&app).context("view receives keys")?;
    let shown = bindings::menu(place, app.prefix());
    let grid = draw::which_key_grid(&app, &shown);
    let buffer = testing::buffer(&app)?;
    assert_eq!(
        buffer[(u16::try_from(grid.x)?, u16::try_from(grid.y)?)].symbol(),
        "╭"
    );
    let title = (grid.x..grid.x + grid.width)
        .map(|x| {
            buffer[(
                u16::try_from(x).unwrap_or(u16::MAX),
                u16::try_from(grid.y).unwrap_or(u16::MAX),
            )]
                .symbol()
        })
        .collect::<String>();
    assert!(title.contains("Space"), "{title:?}");
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
    handle_key(&mut app, key('z'));
    handle_key(&mut app, key('j'));
    let width = app.column_width();
    let sidebar = app.sidebar_width();
    let bar_row = app.text_bar_row();
    let col = (0..width)
        .find(|&c| draw::bar::text_bar(&app).action_at(width, c) == Some(Action::Comment))
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
    app.open_review();
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

    // Lifecycle counts are passive; the title menu owns these settings.
    app.thread_toggle_resolved();
    let header = header::review_header(&app);
    assert!(
        (0..width).all(|column| header.action_at(width, column).is_none()),
        "review scope and counts are passive"
    );
    Ok(())
}

/// The status line has no waiting count; its thread total focuses the pane.
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
        "test:viewer".to_owned(),
        false,
        None,
        None,
    )
    .map_err(anyhow::Error::msg)?;
    let parts = draw::status_parts(&app);
    let text = parts.right_text();
    assert!(!text.contains("waiting"), "{text}");
    let start = app.size().0 - parts.right_width();
    let status_row = app.pane_rows();
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
        .position(|row| after(row).starts_with("▎  ● ▾"))
        .with_context(|| format!("the header with its chevron: {rows:?}"))?;

    // The chevron cell folds and expands.
    left(&mut app, edge + 5, header_y);
    assert!(app.review_list().is_thread_folded(&id), "the chevron folds");
    let rows = testing::screen(&app)?;
    assert!(
        after(&rows[header_y]).starts_with("▎  ● ▸"),
        "the folded row draws `▸`: {rows:?}"
    );
    left(&mut app, edge + 5, header_y);
    assert!(
        !app.review_list().is_thread_folded(&id),
        "the chevron expands"
    );

    // One click on the words selects; two fold and end the gesture.
    left(&mut app, edge + 70, header_y);
    assert!(
        !app.review_list().is_thread_folded(&id),
        "one click selects"
    );
    left(&mut app, edge + 70, header_y);
    assert!(
        app.review_list().is_thread_folded(&id),
        "a double-click folds"
    );
    left(&mut app, edge + 70, header_y);
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
/// files pane's menu keeps only actions local to the pointed item.
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
    assert_eq!(labels, ["fold", "fold all", "open file"]);
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

    // The files pane offers only actions on the pointed file.
    let readme_row = (0..app.tree_rows())
        .find(|&row| {
            app.tree()
                .and_then(|tree| tree.rows().get(row))
                .is_some_and(|r| r.name() == "README.md")
        })
        .context("README in the tree")?;
    right(&mut app, 2, readme_row + 1);
    let labels: Vec<String> = entries(&app)?.into_iter().map(|(_, label)| label).collect();
    assert_eq!(
        labels,
        ["open", "file comment", "copy path", "copy full path"]
    );
    let cell = entry_cell(&app, "file comment")?;
    left(&mut app, cell.0, cell.1);
    assert!(matches!(
        app.popup(),
        Some(Popup::Compose(compose)) if compose.target() == &ComposeTarget::OnFile
    ));
    app.close_popup();

    let docs_row = (0..app.tree_rows())
        .find(|&row| {
            app.tree()
                .and_then(|tree| tree.rows().get(row))
                .is_some_and(|r| r.name() == "docs")
        })
        .context("docs in the tree")?;
    right(&mut app, 2, docs_row + 1);
    let labels: Vec<String> = entries(&app)?.into_iter().map(|(_, label)| label).collect();
    assert_eq!(labels, ["expand", "copy path", "copy full path"]);
    let full = testing::root(&dir)
        .join("docs")
        .to_string_lossy()
        .into_owned();
    let cell = entry_cell(&app, "copy full path")?;
    assert_eq!(left(&mut app, cell.0, cell.1), Effect::Copy(full));
    Ok(())
}

#[test]
fn the_threads_title_opens_checked_settings_below_the_header() -> anyhow::Result<()> {
    let dir = fixture("pane")?;
    let mut app = app(&dir)?;
    app.show_tree();
    if app.threads_pane_height() == 0 {
        app.toggle_threads_pane_shown();
    }
    let before = app.sidebar_scope();
    let header_row = app.pane_top() + app.tree_rows() + 1;
    left(&mut app, 1, header_row);
    assert_eq!(
        app.sidebar_scope(),
        before,
        "the title itself is not a toggle"
    );
    assert_eq!(app.focus(), Focus::ThreadsPane);
    assert_eq!(app.menu().map(Menu::title), Some("Threads"));
    let settings = app
        .menu()
        .context("the Threads menu")?
        .entries()
        .iter()
        .map(|entry| (entry.label().to_owned(), entry.checked()))
        .collect::<Vec<_>>();
    assert_eq!(
        settings,
        [
            ("only current file".to_owned(), Some(true)),
            ("show resolved".to_owned(), Some(false)),
        ]
    );
    let grid = app.menu().context("the Threads menu")?.grid_in(
        app.size().0,
        app.pane_top(),
        app.pane_rows(),
    );
    assert_eq!(grid.x, 0);
    assert_eq!(grid.y, header_row + 1);

    let cell = entry_cell(&app, "only current file")?;
    left(&mut app, cell.0, cell.1);
    assert_eq!(
        app.sidebar_scope(),
        crate::app::threads::pane::PaneScope::Workspace
    );

    left(&mut app, 1, header_row);
    let cell = entry_cell(&app, "show resolved")?;
    left(&mut app, cell.0, cell.1);
    assert!(app.review().resolved);

    let header = header::threads_pane_header(&app);
    assert!(
        (0..app.sidebar_width().saturating_sub(1))
            .all(|column| header.action_at(app.sidebar_width() - 1, column).is_none()),
        "scope and counts are passive"
    );
    Ok(())
}

#[test]
fn the_reviews_title_opens_checked_settings_below_the_header() -> anyhow::Result<()> {
    let dir = fixture("reviews-title")?;
    let mut app = app(&dir)?;
    annotate(&mut app)?;
    app.open_review();
    let header_row = app.pane_top();
    let sidebar = app.sidebar_width();
    let screen = testing::screen(&app)?;
    let header_text = &screen[header_row];
    assert!(header_text.contains("Reviews"), "{header_text:?}");
    assert!(header_text.contains("workspace"), "{header_text:?}");

    left(&mut app, sidebar + 1, header_row);
    assert_eq!(app.focus(), Focus::Review);
    assert_eq!(app.menu().map(Menu::title), Some("Reviews"));
    let settings = app
        .menu()
        .context("the Reviews menu")?
        .entries()
        .iter()
        .map(|entry| (entry.label().to_owned(), entry.checked()))
        .collect::<Vec<_>>();
    assert_eq!(
        settings,
        [
            ("open file".to_owned(), None),
            (String::new(), None),
            ("only current file".to_owned(), Some(false)),
            ("show resolved".to_owned(), Some(false)),
        ]
    );
    let grid = app.menu().context("the Reviews menu")?.grid_in(
        app.size().0,
        app.pane_top(),
        app.pane_rows(),
    );
    assert_eq!(grid.x, sidebar);
    assert_eq!(grid.y, header_row + 1);

    let cell = entry_cell(&app, "only current file")?;
    left(&mut app, cell.0, cell.1);
    assert!(app.review().file_only);

    left(&mut app, sidebar + 1, header_row);
    let cell = entry_cell(&app, "show resolved")?;
    left(&mut app, cell.0, cell.1);
    assert!(app.review().resolved);

    let header = header::review_header(&app);
    assert!(
        (0..app.column_width())
            .all(|column| { header.action_at(app.column_width(), column).is_none() }),
        "scope and counts are passive"
    );
    left(&mut app, sidebar + header.left_width() + 1, header_row);
    assert!(
        app.menu().is_none(),
        "passive header cells do not open a menu"
    );

    left(&mut app, sidebar + 1, header_row);
    let cell = entry_cell(&app, "open file")?;
    left(&mut app, cell.0, cell.1);
    assert!(!app.review_list().is_open());
    assert_eq!(app.focus(), Focus::View);
    Ok(())
}

#[test]
fn the_file_title_opens_navigation_and_display_settings() -> anyhow::Result<()> {
    let dir = fixture("file-title")?;
    let mut app = app(&dir)?;
    annotate(&mut app)?;
    let header_row = app.text_top() - 1;
    let sidebar = app.sidebar_width();
    let header = header::file_header(&app);
    assert!(header.title_width() < header.left_width());
    let shown = testing::screen(&app)?;
    assert!(shown[header_row].contains("File  README.md"));
    assert!(shown[header_row].contains("● 1 active"));

    left(&mut app, sidebar + header.title_width() + 1, header_row);
    assert!(app.menu().is_none(), "the filename is passive");

    left(&mut app, sidebar + 1, header_row);
    assert_eq!(app.menu().map(Menu::title), Some("File"));
    let settings = app
        .menu()
        .context("the File menu")?
        .entries()
        .iter()
        .map(|entry| (entry.label().to_owned(), entry.checked()))
        .collect::<Vec<_>>();
    assert_eq!(
        settings,
        [
            ("open reviews".to_owned(), None),
            (String::new(), None),
            ("rendered view".to_owned(), Some(true)),
            ("show inline threads".to_owned(), Some(true)),
            ("show resolved threads".to_owned(), Some(false)),
        ]
    );
    let grid =
        app.menu()
            .context("the File menu")?
            .grid_in(app.size().0, app.pane_top(), app.pane_rows());
    assert_eq!(grid.x, sidebar);
    assert_eq!(grid.y, header_row + 1);

    let cell = entry_cell(&app, "rendered view")?;
    left(&mut app, cell.0, cell.1);
    assert!(app.view().source_view());

    left(&mut app, sidebar + 1, header_row);
    let cell = entry_cell(&app, "open reviews")?;
    left(&mut app, cell.0, cell.1);
    assert!(app.review_list().is_open());
    assert_eq!(app.focus(), Focus::Review);
    Ok(())
}

#[test]
fn the_context_menu_draws() -> anyhow::Result<()> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::{Color, Modifier, Style};

    let dir = fixture("draw")?;
    let mut app = app(&dir)?;
    let row = row_of(&app, "gamma")?;
    let (column, screen_row) = at(&app, row, 0);
    right(&mut app, column, screen_row);
    let (x, y) = entry_cell(&app, "select line")?;
    mouse(&mut app, MouseEventKind::Moved, x, y);
    let grid = app
        .menu()
        .context("a context menu is open")?
        .grid(app.size().0, app.size().1);
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let mut theme = draw::Theme::from_core(&core);
    let overlay = Color::Rgb(9, 18, 30);
    theme.menu = Style::default().fg(Color::Gray).bg(overlay);
    theme.mode_normal = Style::default().fg(Color::Black).bg(Color::LightYellow);
    theme.popup_key = Style::default().fg(Color::LightBlue);
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    terminal.draw(|frame| draw::draw(frame, &app, &theme))?;
    let buffer = terminal.backend().buffer();
    let text = buffer
        .content()
        .iter()
        .map(|cell| cell.symbol().to_owned())
        .collect::<String>();
    assert!(text.contains("line 5"), "the title names the line");
    assert!(text.contains("comment on line"));
    assert!(text.contains("select line"));
    assert_eq!(
        buffer[(u16::try_from(grid.x)?, u16::try_from(grid.y)?)].symbol(),
        "╭"
    );
    assert_eq!(
        buffer[(
            u16::try_from(grid.x + grid.width - 1)?,
            u16::try_from(grid.y + grid.height - 1)?,
        )]
            .symbol(),
        "╯"
    );
    let title = &buffer[(u16::try_from(grid.x + 1)?, u16::try_from(grid.y)?)];
    assert_eq!(title.fg, theme.info.fg.unwrap_or(Color::Reset));
    assert_eq!(title.bg, overlay);
    assert!(!title.modifier.contains(Modifier::BOLD));
    assert_ne!(
        title.bg,
        Color::LightYellow,
        "the menu title does not borrow the saturated status pill"
    );
    let label = &buffer[(u16::try_from(grid.x + 1)?, u16::try_from(grid.y + 1)?)];
    assert_eq!(label.symbol(), "c");
    assert_eq!(label.fg, theme.menu.fg.unwrap_or(Color::Reset));
    assert_eq!(label.bg, overlay);
    assert!(!label.modifier.contains(Modifier::BOLD));
    let key = &buffer[(
        u16::try_from(grid.x + grid.width - 2)?,
        u16::try_from(grid.y + 1)?,
    )];
    assert_eq!(key.symbol(), "c");
    assert_eq!(key.fg, theme.info.fg.unwrap_or(Color::Reset));
    assert_eq!(key.bg, overlay);
    assert!(!key.modifier.contains(Modifier::BOLD));
    assert_eq!(
        buffer[(
            u16::try_from(grid.x + grid.width - 3)?,
            u16::try_from(grid.y + 1)?,
        )]
            .symbol(),
        " ",
        "the action and its right-aligned shortcut have a gap"
    );
    Ok(())
}

#[test]
fn context_menu_never_overwrites_the_menu_bar() -> anyhow::Result<()> {
    let dir = fixture("context-below-menu-bar")?;
    let mut app = testing::AppBuilder::new(&dir)
        .options(|mut options| {
            options.menu_bar = true;
            options.sidebar.visible = true;
            options
        })
        .build()?;
    app.resize(100, 7);
    app.open_tree_menu(0, 0, app.pane_top() + 1);
    let menu = app.menu().context("a context menu is open")?;
    let grid = menu.grid_in(app.size().0, app.pane_top(), app.pane_rows());
    assert!(grid.y >= app.pane_top());
    let screen = testing::screen(&app)?;
    assert!(screen[0].contains("Go  Review  Diff"), "{:?}", screen[0]);
    Ok(())
}

#[test]
fn releasing_a_divider_drag_over_the_menu_bar_ends_the_drag() -> anyhow::Result<()> {
    let dir = fixture("drag-over-menu-bar")?;
    let mut app = testing::AppBuilder::new(&dir)
        .options(|mut options| {
            options.menu_bar = true;
            options.sidebar.visible = true;
            options
        })
        .build()?;
    let divider = app.sidebar_width() - 1;
    let pane_row = app.pane_top() + 2;
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        divider,
        pane_row,
    );
    assert!(app.dragging().is_some());
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), divider, 0);
    assert!(app.dragging().is_none());
    Ok(())
}
