use anyhow::Context as _;
use crossterm::event::KeyCode;
use fathomable_core::theme::{BUILTIN_NAMES, Theme as CoreTheme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Position;
use ratatui::style::{Color, Style};

use crate::app::draw::{self, Theme};
use crate::app::testing::{self, press, press_key};
use crate::app::{App, Focus, PickerKind, PickerState};

fn render(app: &App, theme: &Theme) -> anyhow::Result<Buffer> {
    let (width, height) = app.size();
    let mut terminal = Terminal::new(TestBackend::new(
        u16::try_from(width)?,
        u16::try_from(height)?,
    ))?;
    terminal.draw(|frame| draw::draw(frame, app, theme))?;
    Ok(terminal.backend().buffer().clone())
}

/// Inspect the underlying pane even when an overlay covers its selected row.
fn render_files(app: &App, theme: &Theme) -> anyhow::Result<Buffer> {
    let width = app.sidebar_width();
    let rows = app.tree_rows();
    let tree = app.tree().context("files pane")?;
    let mut terminal = Terminal::new(TestBackend::new(
        u16::try_from(width)?,
        u16::try_from(rows)?,
    ))?;
    terminal.draw(|frame| {
        let lines = draw::tree_lines(app, tree, theme, width, rows);
        frame.render_widget(
            ratatui::widgets::Paragraph::new(lines).style(theme.sidebar),
            frame.area(),
        );
    })?;
    Ok(terminal.backend().buffer().clone())
}

fn row_containing(buffer: &Buffer, start: u16, end: u16, text: &str) -> anyhow::Result<u16> {
    (0..buffer.area.height)
        .find(|&y| {
            (start..end)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains(text)
        })
        .with_context(|| format!("no visible row contains {text:?}"))
}

fn assert_selection(buffer: &Buffer, x: u16, y: u16, width: u16, theme: &Theme, active: bool) {
    let style = if active {
        theme.list_active
    } else {
        theme.list_inactive
    };
    assert_eq!(buffer[(x, y)].symbol(), if active { "▎" } else { " " });
    if active {
        assert_eq!(Some(buffer[(x, y)].fg), theme.list_cursor.fg);
    }
    for column in x..x + width {
        assert_eq!(Some(buffer[(column, y)].bg), style.bg, "cell {column},{y}");
    }
}

fn marker_count(buffer: &Buffer) -> usize {
    buffer
        .content
        .iter()
        .filter(|cell| cell.symbol() == "▏")
        .count()
}

fn assert_pane_title(buffer: &Buffer, x: u16, y: u16, name: &str, theme: &Theme, focused: bool) {
    assert_eq!(buffer[(x, y)].symbol(), if focused { "▏" } else { " " });
    let expected = if focused {
        theme.pane_focus.fg
    } else {
        theme.text.fg
    };
    for (offset, character) in name.chars().enumerate() {
        let cell = &buffer[(x + 1 + u16::try_from(offset).unwrap_or(u16::MAX), y)];
        assert_eq!(cell.symbol(), character.to_string());
        assert_eq!(Some(cell.fg), expected, "{name:?} cell {offset}");
    }
}

#[test]
fn pane_titles_show_exactly_one_focus_marker_and_suspend_it_for_overlays() -> anyhow::Result<()> {
    let dir = testing::workspace("pane-focus-titles", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.show_tree();
    app.show_threads_pane();

    for name in BUILTIN_NAMES {
        let theme = Theme::from_core(&CoreTheme::resolve(name, |_| Ok(None))?);
        app.window_files();
        let sidebar = u16::try_from(app.sidebar_width())?;
        let pane_top = u16::try_from(app.pane_top())?;
        let thread_top = u16::try_from(app.pane_top() + app.tree_rows() + 1)?;
        let buffer = render(&app, &theme)?;
        assert_eq!(marker_count(&buffer), 1, "{name}");
        assert_pane_title(&buffer, 0, pane_top, "File list", &theme, true);
        assert_pane_title(&buffer, sidebar, pane_top, "File", &theme, false);
        assert_pane_title(&buffer, 0, thread_top, "Thread list", &theme, false);

        app.focus_pane(Focus::View);
        let buffer = render(&app, &theme)?;
        assert_eq!(marker_count(&buffer), 1, "{name}");
        assert_pane_title(&buffer, sidebar, pane_top, "File", &theme, true);
        let filename_x = sidebar + u16::try_from(" File  ".len())?;
        assert_ne!(Some(buffer[(filename_x, pane_top)].fg), theme.pane_focus.fg);

        app.open_review();
        let buffer = render(&app, &theme)?;
        assert_eq!(marker_count(&buffer), 1, "{name}");
        assert_pane_title(&buffer, sidebar, pane_top, "Threads", &theme, true);

        app.open_status();
        let buffer = render(&app, &theme)?;
        assert_eq!(marker_count(&buffer), 0, "{name}");
        app.close_popup();
        app.close_review();
    }
    Ok(())
}

#[test]
fn empty_file_and_thread_lists_keep_their_focused_headers() -> anyhow::Result<()> {
    let dir = testing::bare("empty-pane-focus")?;
    let mut app = testing::AppBuilder::new(&dir).unopened().build()?;
    let theme = Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
    app.show_tree();
    app.show_threads_pane();

    app.window_files();
    let buffer = render(&app, &theme)?;
    assert_pane_title(
        &buffer,
        0,
        u16::try_from(app.pane_top())?,
        "File list",
        &theme,
        true,
    );

    app.window_threads();
    let buffer = render(&app, &theme)?;
    assert_pane_title(
        &buffer,
        0,
        u16::try_from(app.pane_top() + app.tree_rows() + 1)?,
        "Thread list",
        &theme,
        true,
    );

    app.open_review();
    let buffer = render(&app, &theme)?;
    assert_pane_title(
        &buffer,
        u16::try_from(app.sidebar_width())?,
        u16::try_from(app.pane_top())?,
        "Threads",
        &theme,
        true,
    );
    app.open_review_view(crate::app::threads::list::ReviewView::RecentlyResolved);
    let history = render(&app, &theme)?;
    assert_pane_title(
        &history,
        u16::try_from(app.sidebar_width())?,
        u16::try_from(app.pane_top())?,
        "Threads",
        &theme,
        true,
    );

    app.close_review();
    app.focus_pane(Focus::View);
    let file = render(&app, &theme)?;
    assert_pane_title(
        &file,
        u16::try_from(app.sidebar_width())?,
        u16::try_from(app.pane_top())?,
        "File",
        &theme,
        true,
    );
    Ok(())
}

#[test]
fn directory_and_file_info_surfaces_keep_the_file_focus_header() -> anyhow::Result<()> {
    let dir = testing::workspace("file-surface-focus", testing::README)?;
    std::fs::create_dir_all(testing::root(&dir).join("docs"))?;
    std::fs::write(testing::root(&dir).join("image.bin"), b"\0binary")?;
    let mut app = testing::source_app(&dir)?;
    let theme = Theme::from_core(&CoreTheme::resolve("default-light", |_| Ok(None))?);
    let x = u16::try_from(app.sidebar_width())?;
    let y = u16::try_from(app.pane_top())?;

    app.open(std::path::Path::new("docs"));
    app.focus_pane(Focus::View);
    assert_pane_title(&render(&app, &theme)?, x, y, "File", &theme, true);

    app.open(std::path::Path::new("image.bin"));
    assert!(app.info().is_some());
    assert_pane_title(&render(&app, &theme)?, x, y, "File", &theme, true);
    Ok(())
}

#[test]
fn files_threads_and_review_share_active_and_remembered_selection() -> anyhow::Result<()> {
    let dir = testing::workspace("list-focus-panes", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    app.compose_insert("focus message");
    app.compose_submit();
    app.show_tree();
    app.show_threads_pane();

    for name in BUILTIN_NAMES {
        let theme = Theme::from_core(&CoreTheme::resolve(name, |_| Ok(None))?);
        app.window_files();
        let sidebar = u16::try_from(app.sidebar_width())?;
        let buffer = render(&app, &theme)?;
        let file_y = row_containing(&buffer, 0, sidebar, "README.md")?;
        assert_selection(&buffer, 0, file_y, sidebar - 1, &theme, true);
        assert_eq!(Some(buffer[(0, 0)].bg), theme.header.bg);
        assert_ne!(buffer[(0, file_y)].bg, buffer[(0, 0)].bg);
        assert_eq!(buffer[(sidebar - 1, file_y)].symbol(), "│");
        assert_ne!(Some(buffer[(sidebar - 1, file_y)].bg), theme.list_active.bg);

        app.window_threads();
        let buffer = render(&app, &theme)?;
        assert_selection(&buffer, 0, file_y, sidebar - 1, &theme, false);
        let thread_y = row_containing(&buffer, 0, sidebar, "focus message")?;
        assert_selection(&buffer, 0, thread_y - 1, sidebar - 1, &theme, true);
        assert_selection(&buffer, 0, thread_y, sidebar - 1, &theme, true);

        app.open_review();
        let buffer = render(&app, &theme)?;
        assert_selection(&buffer, 0, thread_y, sidebar - 1, &theme, false);
        let review_y = row_containing(&buffer, sidebar, 100, "L3")?;
        assert_selection(&buffer, sidebar, review_y, 100 - sidebar, &theme, true);
        let message_y = row_containing(&buffer, sidebar, 100, "focus message")?;
        assert_eq!(buffer[(sidebar, message_y)].symbol(), "▎");
        assert_eq!(Some(buffer[(sidebar, message_y)].fg), theme.list_cursor.fg);
        assert_eq!(Some(buffer[(sidebar, message_y)].bg), theme.thread_user.bg);
        assert_eq!(Some(buffer[(99, message_y)].bg), theme.thread_user.bg);
        let parent_y = row_containing(&buffer, sidebar, 100, "README.md")?;
        assert_eq!(buffer[(sidebar, parent_y)].symbol(), "▎");
        assert_eq!(Some(buffer[(sidebar, parent_y)].fg), theme.info.fg);

        app.window_files();
        let buffer = render(&app, &theme)?;
        assert_selection(&buffer, sidebar, review_y, 100 - sidebar, &theme, false);
        assert_eq!(buffer[(sidebar, message_y)].symbol(), " ");
        assert_eq!(Some(buffer[(99, message_y)].bg), theme.thread_user.bg);
        app.close_review();
    }
    Ok(())
}

#[test]
fn overlays_and_key_prefixes_release_the_files_cursor_without_moving_it() -> anyhow::Result<()> {
    let dir = testing::workspace("list-focus-overlays", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    let theme = Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
    app.window_files();
    let sidebar = u16::try_from(app.sidebar_width())?;
    let before = app.tree().context("files pane")?.cursor();
    let y = u16::try_from(before + 1)?;

    for keys in [" ", " ?", " f", " Fr", " Fi"] {
        press(&mut app, keys);
        assert_eq!(
            super::Navigation::for_pane(&app, Focus::Tree),
            super::Navigation::Inactive,
            "after {keys:?}: popup {:?}, prefix {:?}",
            app.popup(),
            app.prefix()
        );
        let buffer = render_files(&app, &theme)?;
        assert_selection(&buffer, 0, y, sidebar - 1, &theme, false);
        assert_eq!(app.tree().context("files pane")?.cursor(), before);
        press_key(&mut app, KeyCode::Esc);
        let buffer = render(&app, &theme)?;
        assert_selection(&buffer, 0, y, sidebar - 1, &theme, true);
    }

    app.open_status();
    assert_eq!(
        super::Navigation::for_pane(&app, Focus::Tree),
        super::Navigation::Inactive
    );
    app.close_popup();
    app.start_new_comment();
    assert_eq!(
        super::Navigation::for_pane(&app, Focus::Tree),
        super::Navigation::Inactive
    );
    Ok(())
}

#[test]
fn folded_review_rows_and_file_groups_keep_the_same_focus_language() -> anyhow::Result<()> {
    let dir = testing::workspace("list-focus-folds", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    app.compose_insert("folded message");
    app.compose_submit();
    app.show_tree();
    let theme = Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
    app.open_review();
    app.review_fold();
    let x = u16::try_from(app.sidebar_width())?;
    let buffer = render(&app, &theme)?;
    let y = row_containing(&buffer, x, 100, "folded message")?;
    assert_selection(&buffer, x, y, 100 - x, &theme, true);
    app.window_files();
    assert_selection(&render(&app, &theme)?, x, y, 100 - x, &theme, false);

    app.window_right();
    app.review_toggle_fold(std::path::Path::new("README.md"));
    let buffer = render(&app, &theme)?;
    let y = row_containing(&buffer, x, 100, "README.md")?;
    assert_selection(&buffer, x, y, 100 - x, &theme, true);
    app.window_files();
    assert_selection(&render(&app, &theme)?, x, y, 100 - x, &theme, false);
    app.close_review();

    app.window_threads();
    app.threads_pane_toggle_scope();
    let buffer = render(&app, &theme)?;
    let top = u16::try_from(app.tree_rows())?;
    // Workspace scope adds a file row; clicking it folds and selects the group.
    testing::click(&mut app, 3, usize::from(top + 2));
    let folded = render(&app, &theme)?;
    assert_selection(&folded, 0, top + 2, x - 1, &theme, true);
    assert_ne!(buffer, folded);
    app.window_files();
    assert_selection(&render(&app, &theme)?, 0, top + 2, x - 1, &theme, false);
    Ok(())
}

#[test]
fn the_files_cursor_keeps_git_marks_and_mouse_navigation() -> anyhow::Result<()> {
    let dir = testing::workspace("list-focus-git", testing::README)?;
    let root = testing::root(&dir);
    fathomable_testing::git::init(&root)?;
    fathomable_testing::git::commit_and_stage(&root, &[("README.md", testing::README)])?;
    std::fs::write(root.join("README.md"), "# Changed\n")?;
    std::fs::write(root.join("second.rs"), "fn main() {}\n")?;
    let mut app = testing::source_app(&dir)?;
    app.window_files();
    let theme = Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
    let width = u16::try_from(app.sidebar_width())?;
    let buffer = render(&app, &theme)?;
    let y = row_containing(&buffer, 0, width, "README.md")?;
    assert_selection(&buffer, 0, y, width - 1, &theme, true);
    assert_eq!(buffer[(1, y)].symbol(), "M");
    assert_eq!(buffer[(2, y)].symbol(), " ");
    assert_eq!(Some(buffer[(1, y)].fg), theme.git_unstaged.fg);
    let second_y = row_containing(&buffer, 0, width, "second.rs")?;
    assert_eq!(buffer[(1, second_y)].symbol(), "A");
    assert_eq!(buffer[(2, second_y)].symbol(), " ");
    assert_eq!(Some(buffer[(1, second_y)].fg), theme.diff_plus.fg);
    testing::click(&mut app, 0, usize::from(second_y));
    assert_eq!(app.current_path(), std::path::Path::new("second.rs"));
    assert_selection(&render(&app, &theme)?, 0, second_y, width - 1, &theme, true);
    press_key(&mut app, KeyCode::Up);
    assert_eq!(app.current_path(), std::path::Path::new("README.md"));
    Ok(())
}

#[test]
fn deleted_files_show_red_letters_and_removed_counts() -> anyhow::Result<()> {
    let dir = testing::workspace("list-focus-deleted", testing::README)?;
    let root = testing::root(&dir);
    fathomable_testing::git::init(&root)?;
    fathomable_testing::git::commit_and_stage(
        &root,
        &[
            ("README.md", testing::README),
            ("main.c", "int main() {\n    return 0;\n}\n"),
        ],
    )?;
    let mut app = testing::source_app(&dir)?;
    app.window_files();
    let theme = Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
    let width = u16::try_from(app.sidebar_width())?;
    let buffer = render(&app, &theme)?;
    let y = row_containing(&buffer, 0, width, "main.c -3")?;
    assert_eq!(buffer[(1, y)].symbol(), "D");
    assert_eq!(buffer[(2, y)].symbol(), " ");
    assert_eq!(Some(buffer[(1, y)].fg), theme.diff_minus.fg);
    testing::click(&mut app, 0, usize::from(y));
    assert_eq!(app.current_path(), std::path::Path::new("main.c"));
    assert_selection(&render(&app, &theme)?, 0, y, width - 1, &theme, true);
    assert_eq!(app.banner(), Some("deleted in comparison · showing base"));
    assert_eq!(app.view().text(), "int main() {\n    return 0;\n}\n");
    assert!(app.info().is_none());

    fathomable_testing::git::stage(&root, &[("README.md", testing::README)])?;
    app.on_events(vec![crate::app::watch::Event::Change(
        root.join(".git/index"),
    )]);
    app.settle_status();
    let buffer = render(&app, &theme)?;
    assert_eq!(buffer[(1, y)].symbol(), "D");
    assert_eq!(buffer[(2, y)].symbol(), " ");
    assert_eq!(Some(buffer[(1, y)].fg), theme.diff_minus.fg);
    assert_eq!(app.banner(), Some("deleted in comparison · showing base"));
    assert_eq!(app.view().text(), "int main() {\n    return 0;\n}\n");
    Ok(())
}

#[test]
fn context_menu_hover_is_not_a_keyboard_cursor() -> anyhow::Result<()> {
    use crate::app::input::mouse::handle_mouse;
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let dir = testing::workspace("list-focus-hover", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.window_files();
    let theme = Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
    let event = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    handle_mouse(
        &mut app,
        event(MouseEventKind::Down(MouseButton::Right), 2, 1),
    );
    let (width, height) = app.size();
    let grid = app.menu().context("context menu")?.grid(width, height);
    let x = u16::try_from(grid.x + 1)?;
    let y = u16::try_from(grid.y + 1)?;
    handle_mouse(&mut app, event(MouseEventKind::Moved, x, y));
    let buffer = render(&app, &theme)?;
    assert_eq!(Some(buffer[(x, y)].bg), theme.list_hover.bg);
    assert!(
        !buffer
            .content
            .iter()
            .any(|cell| { cell.symbol() == "▎" && Some(cell.fg) == theme.list_cursor.fg })
    );
    let files = render_files(&app, &theme)?;
    assert_selection(
        &files,
        0,
        1,
        u16::try_from(app.sidebar_width())? - 1,
        &theme,
        false,
    );
    press_key(&mut app, KeyCode::Esc);
    assert_selection(
        &render_files(&app, &theme)?,
        0,
        1,
        u16::try_from(app.sidebar_width())? - 1,
        &theme,
        true,
    );
    Ok(())
}

#[test]
fn every_picker_uses_the_shared_cursor_and_full_width_band() -> anyhow::Result<()> {
    for name in BUILTIN_NAMES {
        let theme = Theme::from_core(&CoreTheme::resolve(name, |_| Ok(None))?);
        for kind in [
            PickerKind::Files,
            PickerKind::AllFiles,
            PickerKind::Recent,
            PickerKind::ComparisonBase,
            PickerKind::ComparisonTarget,
            PickerKind::ComparisonTags(crate::app::ComparisonSide::Base),
            PickerKind::ComparisonBranches(crate::app::ComparisonSide::Base),
            PickerKind::ComparisonBranchCommits(crate::app::ComparisonSide::Base),
            PickerKind::ComparisonReviewPoints,
            PickerKind::ComparisonAdvanced(crate::app::ComparisonSide::Base),
            PickerKind::ReviewPointName,
            PickerKind::Worktree,
        ] {
            let picker = PickerState::new(kind, vec!["candidate".to_owned()]);
            let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
            terminal.draw(|frame| draw::draw_picker(frame, &theme, frame.area(), &picker, None))?;
            let buffer = terminal.backend().buffer();
            // The ordinary 100-column terminal's picker is 90 cells wide.
            let y = row_containing(buffer, 5, 95, "candidate")?;
            assert_selection(buffer, 6, y, 88, &theme, true);
            assert_ne!(theme.list_active.bg, theme.list_hover.bg);
        }
    }
    Ok(())
}

#[test]
fn a_comparison_picker_cursor_follows_the_typed_query() -> anyhow::Result<()> {
    let dir = testing::workspace("comparison-picker-cursor", testing::README)?;
    let mut app = testing::app(&dir)?;
    app.open_picker(PickerKind::ComparisonBase);
    for ch in "main".chars() {
        app.picker_char(ch);
    }
    let core = CoreTheme::resolve("default-dark", |_| Ok(None))?;
    let theme = Theme::from_core(&core);
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    terminal.draw(|frame| draw::draw(frame, &app, &theme))?;
    let cursor = terminal.get_cursor_position()?;
    let buffer = terminal.backend().buffer();
    let row = row_containing(buffer, 0, 100, "comparison base > main")?;
    let input = (0..=96_u16)
        .find(|&x| {
            (0..4)
                .map(|offset| buffer[(x + offset, row)].symbol())
                .collect::<String>()
                == "main"
        })
        .context("typed picker query")?;
    assert_eq!(
        cursor,
        Position::new(input + 4, row),
        "the cursor rests where the next character will be inserted"
    );
    assert_eq!(buffer[(input + 3, row)].symbol(), "n");
    assert_eq!(buffer[(input + 4, row)].symbol(), " ");
    Ok(())
}

#[test]
fn explicit_semantic_backgrounds_do_not_split_selected_rows() -> anyhow::Result<()> {
    let dir = testing::workspace("list-focus-theme", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    app.compose_insert("theme message");
    app.compose_submit();
    app.open_review();
    let mut theme = Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
    theme.info = Style::default().fg(Color::White).bg(Color::Red);
    theme.thread_active = Style::default().fg(Color::Green).bg(Color::Magenta);
    let buffer = render(&app, &theme)?;
    let x = u16::try_from(app.sidebar_width())?;
    let y = row_containing(&buffer, x, 100, "L3")?;
    assert_selection(&buffer, x, y, 100 - x, &theme, true);
    assert!(buffer.content.iter().any(|cell| {
        cell.symbol() == "●" && cell.fg == Color::Green && Some(cell.bg) == theme.list_active.bg
    }));
    theme.list_active = theme.list_active.fg(Color::White);
    theme.list_cursor = theme.list_cursor.bg(Color::Yellow);
    assert_selection(&render(&app, &theme)?, x, y, 100 - x, &theme, true);
    Ok(())
}
