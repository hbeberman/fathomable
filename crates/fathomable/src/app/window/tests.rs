use std::path::Path;

use anyhow::Context as _;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::input::{keys, mouse};
use crate::app::testing::{self, press, press_key, source_app};
use crate::app::{App, Deleted, Focus, PickerKind, Popup};

fn annotate(app: &mut App, line: usize, text: &str) {
    app.view_mut().goto_source_line(line);
    app.start_new_comment();
    app.compose_insert(text);
    app.compose_submit();
}

#[test]
fn file_list_fold_keys_toggle_directories_without_opening_files() -> anyhow::Result<()> {
    let dir = testing::workspace("file-list-fold-keys", testing::README)?;
    std::fs::create_dir_all(testing::root(&dir).join("docs/nested"))?;
    std::fs::write(testing::root(&dir).join("docs/nested/note.md"), "note\n")?;
    let mut app = source_app(&dir)?;
    press(&mut app, "FGgg");
    assert_eq!(app.directory_path(), Some(Path::new("docs")));
    press(&mut app, "z");
    assert!(
        app.tree()
            .context("tree")?
            .contains(Path::new("docs/nested"))
    );
    assert_eq!(app.focus(), Focus::Tree);
    assert_eq!(app.directory_path(), Some(Path::new("docs")));
    press(&mut app, "z");
    assert!(
        !app.tree()
            .context("tree")?
            .contains(Path::new("docs/nested"))
    );
    press(&mut app, "Z");
    assert!(
        app.tree()
            .context("tree")?
            .contains(Path::new("docs/nested/note.md"))
    );
    press(&mut app, "Z");
    assert!(
        !app.tree()
            .context("tree")?
            .contains(Path::new("docs/nested"))
    );
    press(&mut app, "Gz");
    assert_eq!(app.focus(), Focus::Tree, "z on a file never opens it");
    assert_eq!(app.current_path(), Path::new("README.md"));
    app.open_review();
    press(&mut app, "FZ");
    assert!(app.review_list().is_open());
    assert_eq!(app.focus(), Focus::Tree);
    Ok(())
}

fn wheel(app: &mut App, column: usize, row: usize) -> anyhow::Result<()> {
    mouse::handle_mouse(
        app,
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: u16::try_from(column)?,
            row: u16::try_from(row)?,
            modifiers: KeyModifiers::NONE,
        },
    );
    Ok(())
}

#[test]
fn hidden_draft_ignores_keys_mouse_paste_and_direct_actions() -> anyhow::Result<()> {
    let dir = testing::workspace("focus-hidden-draft", testing::README)?;
    let mut app = source_app(&dir)?;
    press(&mut app, "ckeep this draft");
    let text = app.draft().context("draft")?.buffer().text().to_owned();
    let cursor = app.draft().context("draft")?.buffer().cursor();
    app.resize(12, 2);
    press(&mut app, "ignored");
    app.paste("ignored paste");
    press_key(&mut app, KeyCode::Enter);
    press_key(&mut app, KeyCode::Backspace);
    app.act(crate::app::input::bindings::Action::Confirm);
    testing::click(&mut app, 5, 1);
    wheel(&mut app, 5, 1)?;
    assert_eq!(app.draft().context("kept draft")?.buffer().text(), text);
    assert_eq!(app.draft().context("kept draft")?.buffer().cursor(), cursor);
    assert!(app.file_threads().is_empty());

    app.resize(100, 30);
    press_key(&mut app, KeyCode::Enter);
    assert!(app.draft().is_none());
    assert_eq!(app.file_threads().len(), 1);
    Ok(())
}

#[test]
fn hidden_confirmation_cannot_archive_the_board() -> anyhow::Result<()> {
    let dir = testing::workspace("focus-hidden-confirmation", testing::README)?;
    let mut app = source_app(&dir)?;
    annotate(&mut app, 3, "keep this thread");
    let id = app.file_threads()[0].clone();
    app.request_clear_board();
    assert!(matches!(app.popup(), Some(Popup::ConfirmBoard { .. })));
    app.resize(40, 2);
    press_key(&mut app, KeyCode::Enter);
    testing::click(&mut app, 5, 1);
    assert!(!app.thread(&id).context("retained thread")?.is_archived());
    assert!(matches!(app.popup(), Some(Popup::ConfirmBoard { .. })));
    press_key(&mut app, KeyCode::Esc);
    assert!(app.popup().is_none());
    press(&mut app, "q");
    assert!(matches!(app.popup(), Some(Popup::ConfirmQuit)));
    assert!(matches!(
        keys::handle_key(
            &mut app,
            crossterm::event::KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ),
        crate::app::view::Effect::Quit
    ));
    Ok(())
}

#[test]
fn hidden_picker_and_command_keep_their_input_without_activation() -> anyhow::Result<()> {
    let dir = testing::workspace("focus-hidden-input", testing::README)?;
    std::fs::write(testing::root(&dir).join("second.rs"), "second\n")?;
    let mut app = source_app(&dir)?;
    app.open_picker(PickerKind::Files);
    app.resize(12, 2);
    press(&mut app, "second");
    press_key(&mut app, KeyCode::Down);
    press_key(&mut app, KeyCode::Enter);
    let Some(Popup::Picker(picker)) = app.popup() else {
        anyhow::bail!("hidden picker was activated");
    };
    assert_eq!(picker.input(), "");
    assert_eq!(picker.selected(), 0);
    assert_eq!(app.current_path(), Path::new("README.md"));

    press_key(&mut app, KeyCode::Esc);
    app.resize(100, 30);
    press(&mut app, ":5");
    app.resize(12, 2);
    press(&mut app, "99");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.view().input(), "5");
    assert_eq!(app.view().mode(), crate::app::view::Mode::Command);
    press_key(&mut app, KeyCode::Esc);
    assert_eq!(app.view().mode(), crate::app::view::Mode::Normal);
    Ok(())
}

#[test]
fn file_wheel_preserves_selection_and_shared_thread_cursor() -> anyhow::Result<()> {
    let text = "source line\n".repeat(120);
    let dir = testing::workspace("focus-file-wheel", &text)?;
    let mut app = source_app(&dir)?;
    annotate(&mut app, 3, "first");
    annotate(&mut app, 80, "second");
    app.view_mut().goto_source_line(3);
    press(&mut app, "vl");
    let source = app.view().source_position();
    let selection = app.view().selected_source();
    let thread = app.thread_cursor().thread().cloned();
    let row = app.text_top();
    for _ in 0..25 {
        wheel(&mut app, 20, row)?;
    }
    assert!(app.view().scroll() > app.view().cursor().row);
    assert_eq!(app.view().source_position(), source);
    assert_eq!(app.view().selected_source(), selection);
    assert_eq!(app.thread_cursor().thread(), thread.as_ref());
    app.resize(80, 25);
    assert_eq!(app.view().source_position(), source);
    assert_eq!(app.view().selected_source(), selection);
    assert!(app.view().scroll() > app.view().cursor().row);
    press(&mut app, "l");
    assert!(app.view().scroll() <= app.view().cursor().row);
    Ok(())
}

#[test]
fn preview_does_not_resume_a_parked_draft_or_take_input() -> anyhow::Result<()> {
    let dir = testing::workspace("focus-preview-draft", testing::README)?;
    let mut app = source_app(&dir)?;
    annotate(&mut app, 3, "existing thread");
    app.start_new_comment();
    app.compose_insert("parked text");
    app.park_draft();
    app.open_review();
    press(&mut app, "Tj");
    assert_eq!(app.focus(), Focus::ThreadsPane);
    assert!(app.review_list().is_open());
    assert!(app.popup().is_none(), "preview must not claim draft input");
    press_key(&mut app, KeyCode::Right);
    press(&mut app, "l");
    assert_eq!(app.focus(), Focus::ThreadsPane);
    assert!(app.review_list().is_open());
    assert!(app.popup().is_none());
    press_key(&mut app, KeyCode::Enter);
    assert!(!app.review_list().is_open());
    assert_eq!(app.focus(), Focus::View);
    assert_eq!(
        app.draft()
            .context("explicitly resumed draft")?
            .buffer()
            .text(),
        "parked text"
    );
    Ok(())
}

#[test]
fn resize_and_refocus_preserve_selection_and_independent_list_scroll() -> anyhow::Result<()> {
    let dir = testing::workspace("focus-resize-state", testing::README)?;
    for index in 0..60 {
        std::fs::write(
            testing::root(&dir).join(format!("file-{index:02}.rs")),
            "text\n",
        )?;
    }
    let mut app = source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    press(&mut app, "vll");
    let selected = app.view().selected_source();
    let cursor = app.view().source_position();
    app.resize(12, 2);
    app.resize(100, 30);
    assert_eq!(app.view().selected_source(), selected);
    assert_eq!(app.view().source_position(), cursor);
    app.resize(40, 12);
    assert_eq!(app.view().selected_source(), selected);
    app.resize(100, 30);
    assert_eq!(app.view().selected_source(), selected);

    press_key(&mut app, KeyCode::Esc);
    press(&mut app, "F");
    let row = app.pane_top() + 1;
    for _ in 0..8 {
        wheel(&mut app, 2, row)?;
    }
    let scroll = app.tree_scroll();
    assert!(scroll > 0);
    let selection = app.tree().context("File list")?.cursor();
    press(&mut app, "fFf");
    assert_eq!(app.tree_scroll(), scroll);
    assert_eq!(app.tree().context("File list")?.cursor(), selection);
    Ok(())
}

#[test]
fn directory_content_survives_focus_changes_and_cannot_edit_the_hidden_file() -> anyhow::Result<()>
{
    let dir = testing::workspace("focus-directory", testing::README)?;
    std::fs::create_dir(testing::root(&dir).join("docs"))?;
    std::fs::write(testing::root(&dir).join("docs/note.md"), "note\n")?;
    let mut app = source_app(&dir)?;
    press(&mut app, "F");
    let index = app
        .tree()
        .context("File list")?
        .rows()
        .iter()
        .position(|row| row.path() == Path::new("docs"))
        .context("directory row")?;
    let row = app.pane_top() + 1 + index - app.tree_scroll();
    testing::click(&mut app, 3, row);
    assert_eq!(app.directory_path(), Some(Path::new("docs")));
    let cursor = app.view().source_position();
    press(&mut app, "w");
    assert_eq!(app.focus(), Focus::View);
    assert_eq!(app.directory_path(), Some(Path::new("docs")));
    press(&mut app, "jcv");
    assert!(app.popup().is_none());
    assert_eq!(app.view().source_position(), cursor);
    press(&mut app, ":1");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(
        app.view().source_position(),
        cursor,
        "directory commands cannot move hidden source"
    );
    let text_row = app.text_top();
    testing::click(&mut app, 40, text_row);
    mouse::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 40,
            row: u16::try_from(text_row)?,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(app.popup().is_none());
    assert_eq!(app.view().source_position(), cursor);
    press(&mut app, "F");
    press_key(&mut app, KeyCode::Esc);
    assert_eq!(app.directory_path(), Some(Path::new("docs")));
    app.open(Path::new("README.md"));
    assert!(app.directory_path().is_none());
    Ok(())
}

#[test]
fn minimum_layout_allocates_real_content_even_with_banner_and_small_split() -> anyhow::Result<()> {
    let dir = testing::workspace("focus-minimum-content", testing::README)?;
    let mut app = source_app(&dir)?;
    app.toggle_menu_bar();
    let index = app.current.context("document")?;
    app.docs[index].deleted = Some(Deleted::Loaded);
    app.resize(20, 5);
    assert!(app.panes_fit());
    assert!(app.banner().is_some());
    assert!(
        !app.text_bar_shown(),
        "banner must not displace the only data row"
    );
    assert_eq!(app.text_rows(), 1);

    app.resize(100, 30);
    press(&mut app, "FT");
    app.sidebar.split = Some(1);
    app.resize(28, 8);
    assert!(app.panes_fit());
    assert_eq!(app.column_width(), 20);
    assert_eq!(app.tree_rows(), 2);
    assert_eq!(app.threads_pane_height(), 4);
    assert_eq!(app.threads_pane_body_rows(), 1);
    Ok(())
}

#[test]
fn thread_preview_reveals_main_threads_without_changing_folds_or_focus() -> anyhow::Result<()> {
    let text = "line\n".repeat(30);
    let dir = testing::workspace("focus-main-preview", &text)?;
    let mut app = source_app(&dir)?;
    for line in 1..=14 {
        annotate(
            &mut app,
            line,
            &format!("discussion-{line:02}\nextra message line"),
        );
    }
    let first = app.file_threads()[0].clone();
    app.open_review();
    app.set_thread_cursor(first);
    press(&mut app, "ggT");
    for _ in 0..11 {
        press(&mut app, "j");
    }
    assert!(app.review_list().scroll() > 0);
    assert_eq!(app.focus(), Focus::ThreadsPane);
    assert!(app.review_list().is_open());
    let buffer = testing::buffer(&app)?;
    let sidebar = u16::try_from(app.sidebar_width())?;
    let main = (0..buffer.area.height)
        .map(|row| {
            (sidebar..buffer.area.width)
                .map(|column| buffer[(column, row)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(main.contains("discussion-12"), "{main}");
    app.review_toggle_fold(Path::new("README.md"));
    let review = app.review();
    press(&mut app, "j");
    assert!(app.review_list().is_folded(Path::new("README.md")));
    assert_eq!(app.review_list().scroll(), 0, "reveal the folded file row");
    assert_eq!(app.review(), review, "preview retains filters");
    assert_eq!(app.focus(), Focus::ThreadsPane);
    Ok(())
}
