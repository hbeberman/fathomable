use std::fs;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fathomable_core::annotations::{Author, Reply, Thread};
use fathomable_testing::TempDir;

use crate::app::testing::{self, press, source_app};

use super::{PaneEntry, PaneRow, PaneScope};
use crate::app::input::keys;
use crate::app::threads::stubs::Stub;
use crate::app::threads::{ComposeTarget, ThreadState};
use crate::app::{App, Border, Focus, Popup};

fn fixture(name: &str) -> std::io::Result<TempDir> {
    let dir = testing::workspace(&format!("threads-pane-{name}"), testing::README)?;
    fs::create_dir_all(dir.0.join("ws/docs"))?;
    fs::write(dir.0.join("ws/docs/guide.md"), "# Guide\n\nfirst\nsecond\n")?;
    Ok(dir)
}

fn annotate(app: &mut App, line: usize, text: &str) {
    app.view_mut().goto_source_line(line);
    app.start_new_comment();
    app.compose_insert(text);
    app.compose_submit();
}

/// A user reply to the thread of mark `index`, dated after every
/// thread's `updated`.
fn answer_later(app: &mut App, index: usize) -> anyhow::Result<()> {
    let id = app.marks()[index].id().clone();
    let store = app.store_mut().ok_or_else(|| anyhow::anyhow!("no store"))?;
    let later = store
        .threads()
        .iter()
        .map(Thread::modified)
        .max()
        .unwrap_or(0)
        + 1;
    store.reply(&id, Reply::new(Author::User, later, "later"))?;
    Ok(())
}

fn sidebar_column(app: &App) -> anyhow::Result<Vec<String>> {
    let (_, buffer) = sidebar_buffer(app)?;
    Ok((0..buffer.area.height)
        .map(|y| {
            (0..u16::try_from(app.sidebar_width()).unwrap_or(0))
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect())
}

fn sidebar_buffer(app: &App) -> anyhow::Result<(crate::app::draw::Theme, ratatui::buffer::Buffer)> {
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))?;
    terminal.draw(|frame| crate::app::draw::draw(frame, app, &theme))?;
    let buffer = terminal.backend().buffer().clone();
    Ok((theme, buffer))
}

/// The paths of the pane's file rows, `▸` before a folded one.
fn file_rows(app: &App) -> Vec<String> {
    app.threads_pane_rows()
        .iter()
        .filter_map(|row| match row {
            PaneRow::File { path, folded, .. } => Some(format!(
                "{}{}",
                if *folded { "▸ " } else { "" },
                path.display()
            )),
            PaneRow::Thread(_) => None,
        })
        .collect()
}

/// Two threads on the README and one on the guide, the README open
/// with the cursor on L3, both panes shown.
fn three_threads(name: &str) -> anyhow::Result<(TempDir, App)> {
    let dir = fixture(name)?;
    let mut app = source_app(&dir)?;
    app.show_tree();
    app.show_threads_pane();
    annotate(&mut app, 7, "seven");
    annotate(&mut app, 3, "three");
    app.open(Path::new("docs/guide.md"));
    annotate(&mut app, 3, "guide first");
    app.open(Path::new("README.md"));
    app.view_mut().goto_source_line(3);
    Ok((dir, app))
}

/// The pane lists this file in line order, two rows per thread with
/// the circle, the place, the author, and the newest message, and
/// hides resolved threads until `x` (ADR 0049, ADR 0066).
#[test]
fn the_pane_lists_the_file_and_hides_resolved() -> anyhow::Result<()> {
    let (_dir, mut app) = three_threads("scope")?;
    let rows = app.threads_pane_entries();
    assert_eq!(
        rows.iter()
            .map(|row| (row.place(), row.summary()))
            .collect::<Vec<_>>(),
        [("L3".to_owned(), "three"), ("L7".to_owned(), "seven")]
    );
    assert!(file_rows(&app).is_empty(), "file scope has no file rows");
    assert_eq!(app.threads_pane_selected(), Some(0));

    // The drawn pane: rule, header with the scope and the counts by
    // colour, two rows per thread.
    let column = sidebar_column(&app)?;
    let top = app.tree_rows();
    assert!(
        column[top].starts_with("───") && column[top].ends_with('┤'),
        "connected rule: {:?}",
        column[top]
    );
    assert!(
        column[top + 1].contains("threads · file") && column[top + 1].contains("● 2"),
        "{:?}",
        column[top + 1]
    );
    assert!(!column[top + 1].contains("s x"), "the keys left the header");
    assert!(
        column[top + 2].contains("● L3 ") && column[top + 2].contains(app.user_name()),
        "{:?}",
        column[top + 2]
    );
    assert!(column[top + 3].contains("three"), "{:?}", column[top + 3]);
    assert!(column[top + 4].contains("● L7 "), "{:?}", column[top + 4]);
    assert!(column[top + 4].contains(" now"), "{:?}", column[top + 4]);
    assert!(column[top + 5].contains("seven"), "{:?}", column[top + 5]);

    let (theme, buffer) = sidebar_buffer(&app)?;
    let divider = u16::try_from(app.sidebar_width())? - 1;
    let divider_bg = theme.sidebar.bg.unwrap_or(ratatui::style::Color::Reset);
    assert_ne!(
        divider_bg,
        theme.header.bg.unwrap_or(ratatui::style::Color::Reset),
        "the test theme must distinguish the sidebar and header surfaces"
    );
    for row in [0, top + 1] {
        assert_eq!(
            buffer[(divider, u16::try_from(row)?)].bg,
            divider_bg,
            "header row {row} must not paint the divider"
        );
    }

    // A reply becomes the summary and earns the reply count.
    app.thread_reply();
    app.compose_insert("answered");
    app.compose_submit();
    assert_eq!(app.threads_pane_entries()[0].summary(), "answered");
    assert_eq!(app.threads_pane_entries()[0].replies(), 1);

    // Resolved threads leave the list until `x` shows them; the
    // header still counts them.
    app.focus_threads_pane();
    assert_eq!(app.focus(), Focus::ThreadsPane);
    press(&mut app, "r");
    assert_eq!(app.marks()[1].kind(), ThreadState::Resolved, "L3 resolved");
    assert_eq!(app.threads_pane_entries().len(), 1);
    assert_eq!(
        app.threads_pane_entries()[0].range().map(|r| r.start()),
        Some(7)
    );
    let column = sidebar_column(&app)?;
    assert!(
        column[top + 1].contains("● 1") && column[top + 1].contains("○ 1"),
        "{:?}",
        column[top + 1]
    );
    // With the keys, the bottom row is the key bar.
    let bar = &column[app.pane_rows() - 1];
    assert!(
        bar.contains("c reply") && bar.contains("r reopen"),
        "{bar:?}"
    );
    assert!(
        !bar.contains("z fold"),
        "z does nothing in file scope: {bar:?}"
    );
    press(&mut app, "x");
    assert_eq!(app.message(), Some("resolved shown"));
    assert_eq!(app.threads_pane_entries().len(), 2);
    assert_eq!(app.threads_pane_entries()[0].kind(), ThreadState::Resolved);
    assert_eq!(app.threads_pane_entries()[0].words().glyph(), "○");
    press(&mut app, "x");
    assert_eq!(app.threads_pane_entries().len(), 1);
    Ok(())
}

/// `s` lists the workspace grouped by file in the files pane's
/// order, the place saying the lines alone; `j` opens the other file
/// and keeps the keys; Enter and `c` act on the cursor's thread
/// (ADR 0049, ADR 0066).
#[test]
fn the_pane_lists_the_workspace_by_file() -> anyhow::Result<()> {
    let (_dir, mut app) = three_threads("workspace")?;
    app.focus_threads_pane();
    press(&mut app, "r");
    assert_eq!(app.marks()[1].kind(), ThreadState::Resolved, "L3 resolved");
    answer_later(&mut app, 0)?;
    press(&mut app, "s");
    assert_eq!(app.sidebar_scope(), PaneScope::Workspace);
    assert_eq!(file_rows(&app), ["docs/guide.md", "README.md"]);
    let rows = app.threads_pane_entries();
    assert_eq!(
        rows.iter().map(PaneEntry::place).collect::<Vec<_>>(),
        ["L3", "L7"]
    );
    let column = sidebar_column(&app)?;
    assert!(
        column[app.tree_rows() + 1].contains("threads · workspace"),
        "{:?}",
        column[app.tree_rows() + 1]
    );
    let file_row = column[app.tree_rows() + 2].trim_end_matches('│').to_owned();
    assert!(
        file_row.contains("docs/guide.md") && file_row.trim_end().ends_with('1'),
        "{file_row:?}"
    );
    let bar = &column[app.pane_rows() - 1];
    assert!(
        bar.contains("r reopen") && bar.contains("z fold"),
        "{bar:?}"
    );

    // `j` steps across files and keeps the keys in the pane.
    app.view_mut().goto_source_line(7);
    press(&mut app, "j");
    assert_eq!(app.current_path(), Path::new("docs/guide.md"));
    assert_eq!(app.view().cursor_source_line(), Some(3));
    assert_eq!(app.focus(), Focus::ThreadsPane);
    assert_eq!(app.threads_pane_selected(), Some(0));
    press(&mut app, "j");
    assert_eq!(app.current_path(), Path::new("README.md"), "wrapped");
    assert_eq!(app.focus(), Focus::ThreadsPane);

    // Enter opens the thread expanded with the keys in the text; `c`
    // replies in place with the keys staying here.
    keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.focus(), Focus::View);
    assert!(app.shows_thread());
    app.focus_threads_pane();
    press(&mut app, "c");
    assert!(
        matches!(app.popup(), Some(Popup::Compose(c)) if matches!(c.target(), ComposeTarget::Reply(_)))
    );
    // The draft is written at the end of the thread's rows in the
    // text, after its header and two one-line messages (ADR 0054).
    assert!(app.shows_thread());
    assert_eq!(app.stubs().iter().find_map(Stub::draft_slot), Some((5, 0)));
    app.compose_insert("ok");
    app.compose_submit();
    assert_eq!(app.focus(), Focus::ThreadsPane);
    Ok(())
}

/// `z` folds the cursor's file to its row and `Z` every file (ADR
/// 0066); a folded file is one stop for `j`, its row highlighted
/// while the cursor is inside it, and the fold outlives a scope
/// switch.
#[test]
fn z_folds_a_file_and_shift_z_every_file() -> anyhow::Result<()> {
    let dir = fixture("fold")?;
    let mut app = source_app(&dir)?;
    app.show_threads_pane();
    annotate(&mut app, 3, "three");
    annotate(&mut app, 7, "seven");
    app.open(Path::new("docs/guide.md"));
    annotate(&mut app, 3, "guide");
    app.open(Path::new("README.md"));
    app.view_mut().goto_source_line(3);
    app.focus_threads_pane();
    press(&mut app, "z");
    assert!(file_rows(&app).is_empty(), "nothing to fold in file scope");

    press(&mut app, "s");
    press(&mut app, "z");
    assert_eq!(file_rows(&app), ["docs/guide.md", "▸ README.md"]);
    assert_eq!(
        app.threads_pane_entries().len(),
        1,
        "README's rows are gone"
    );
    // A file row always carries its arrow (ADR 0076).
    let shown = testing::screen(&app)?;
    assert!(
        shown.iter().any(|row| row.contains("▾ docs/guide.md"))
            && shown.iter().any(|row| row.contains("▸ README.md")),
        "{shown:?}"
    );
    assert!(app.threads_pane_rows().iter().any(|row| matches!(
        row,
        PaneRow::File { path, selected: true, .. } if path == Path::new("README.md")
    )));
    // `k` leaves the folded file for the guide; `j` comes back to
    // README's first thread and one more `j` wraps past its second.
    press(&mut app, "k");
    assert_eq!(app.current_path(), Path::new("docs/guide.md"));
    press(&mut app, "j");
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.view().cursor_source_line(), Some(3));
    press(&mut app, "j");
    assert_eq!(app.current_path(), Path::new("docs/guide.md"), "one stop");
    // `z` on a thread row folds its file; the fold survives `s`.
    press(&mut app, "z");
    assert_eq!(file_rows(&app), ["▸ docs/guide.md", "▸ README.md"]);
    press(&mut app, "s");
    press(&mut app, "s");
    assert_eq!(file_rows(&app), ["▸ docs/guide.md", "▸ README.md"]);
    press(&mut app, "Z");
    assert_eq!(file_rows(&app), ["docs/guide.md", "README.md"]);
    press(&mut app, "Z");
    assert_eq!(file_rows(&app), ["▸ docs/guide.md", "▸ README.md"]);
    Ok(())
}

/// The pane shows with the tree hidden and takes the whole sidebar;
/// beside the tree its split is fixed whatever the thread count, and
/// only a drag changes it. `Space w h` and `Space w l` focus and
/// return; `Space p f` and `Space p t` show and hide each pane on its
/// own (ADR 0056).
#[test]
fn the_sidebar_shows_either_pane_and_the_split_is_fixed() -> anyhow::Result<()> {
    let dir = fixture("split")?;
    let mut app = source_app(&dir)?;
    assert_eq!(app.sidebar_width(), 0);
    assert_eq!(app.threads_pane_height(), 0);

    // With nothing shown the pane alone fills the sidebar.
    app.focus_threads_pane();
    assert_eq!(app.focus(), Focus::ThreadsPane);
    assert!(app.tree().is_none());
    assert_eq!(app.sidebar_width(), 32);
    assert_eq!(app.threads_pane_height(), app.pane_rows());
    assert_eq!(app.tree_rows(), 0);
    let column = sidebar_column(&app)?;
    assert!(column[1].contains("threads · file"), "{:?}", column[1]);
    assert!(
        column[2].contains("no threads in this file"),
        "{:?}",
        column[2]
    );

    // `Space w l` hands the keys back; the pane stays.
    press(&mut app, " wl");
    assert_eq!(app.focus(), Focus::View);
    assert!(app.threads_pane_shown());

    // The tree joins above at the configured split of 8 rows, and a
    // dozen threads do not grow it; `Space p f` shows the files pane
    // and `Space w h` lands on it, the pane to the text's left.
    press(&mut app, " pf");
    press(&mut app, " wh");
    assert_eq!(app.focus(), Focus::Tree);
    assert_eq!(app.threads_pane_height(), 8);
    assert_eq!(app.tree_rows(), app.pane_rows() - 8);
    for line in 1..=8 {
        annotate(&mut app, line, "many");
    }
    assert_eq!(app.threads_pane_entries().len(), 8);
    assert_eq!(app.threads_pane_height(), 8, "the split is fixed");

    // A drag on the rule changes it for the session.
    let mouse = |kind, row: usize| MouseEvent {
        kind,
        column: 2,
        row: u16::try_from(row).unwrap_or(u16::MAX),
        modifiers: KeyModifiers::NONE,
    };
    let top = app.tree_rows();
    crate::app::input::mouse::handle_mouse(
        &mut app,
        mouse(MouseEventKind::Down(MouseButton::Left), top),
    );
    assert_eq!(app.dragging(), Some(Border::ThreadsPane));
    crate::app::input::mouse::handle_mouse(
        &mut app,
        mouse(MouseEventKind::Drag(MouseButton::Left), top - 4),
    );
    crate::app::input::mouse::handle_mouse(
        &mut app,
        mouse(MouseEventKind::Up(MouseButton::Left), top - 4),
    );
    assert_eq!(app.threads_pane_height(), 12);
    assert_eq!(app.tree_rows(), top - 4);

    // `Space p f` hides the tree and leaves the pane; `Space p t` hides
    // the pane, and with both gone the sidebar goes. Both keys show
    // their pane again without taking the keys.
    app.focus_threads_pane();
    press(&mut app, " pf");
    assert!(app.tree().is_none());
    assert!(app.threads_pane_shown());
    assert_eq!(
        app.focus(),
        Focus::ThreadsPane,
        "the keys stay with the pane"
    );
    assert_eq!(app.sidebar_width(), 32);
    press(&mut app, " pt");
    assert!(!app.threads_pane_shown());
    assert_eq!(app.focus(), Focus::View);
    assert_eq!(app.sidebar_width(), 0);
    press(&mut app, " pt");
    assert!(app.threads_pane_shown(), "the same key shows it again");
    assert_eq!(app.focus(), Focus::View, "showing does not take the keys");
    press(&mut app, " pf");
    assert!(app.tree().is_some(), "the same key shows it again");
    assert_eq!(app.focus(), Focus::View, "showing does not take the keys");
    Ok(())
}

/// A step from any surface moves the one cursor, and every surface
/// highlights the same thread (ADR 0046).
#[test]
fn every_surface_shows_the_one_cursor() -> anyhow::Result<()> {
    let dir = fixture("cursor")?;
    let mut app = source_app(&dir)?;
    app.show_tree();
    app.show_threads_pane();
    annotate(&mut app, 2, "two");
    annotate(&mut app, 6, "six");
    annotate(&mut app, 7, "seven");
    let ids = app.file_threads();

    // Nothing open: the cursor rides the text.
    app.view_mut().goto_source_line(6);
    assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));
    assert_eq!(app.threads_pane_selected(), Some(1));
    app.view_mut().goto_source_line(1);
    assert_eq!(app.thread_cursor().thread(), Some(&ids[0]));

    // The text: `]c` steps, the threads pane's highlight follows.
    app.focus_pane(Focus::View);
    app.view_mut().goto_source_line(2);
    app.expand_at_cursor();
    assert_eq!(app.focus(), Focus::View);
    press(&mut app, "]c");
    assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));
    assert_eq!(app.threads_pane_selected(), Some(1));
    assert_eq!(app.thread_position(), Some((2, 3)));

    // The threads pane: `j` steps the same cursor.
    app.focus_threads_pane();
    press(&mut app, "j");
    assert_eq!(app.thread_cursor().thread(), Some(&ids[2]));
    assert_eq!(app.thread_position(), Some((3, 3)));
    assert_eq!(app.view().cursor_source_line(), Some(7));

    // The list opens on it and `k` steps it back.
    app.open_review();
    assert_eq!(app.thread_cursor().thread(), Some(&ids[2]));
    assert_eq!(app.review_selected_index(), Some(2));
    press(&mut app, "k");
    assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));

    // Enter expands it in the text; the cursor lands on its line.
    keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.focus(), Focus::View);
    assert_eq!(app.thread_position(), Some((2, 3)));
    assert_eq!(app.view().cursor_source_line(), Some(6));

    // The cursor stays until the reader moves.
    assert_eq!(app.thread_cursor().thread(), Some(&ids[1]));
    app.view_mut().goto_source_line(7);
    assert_eq!(app.thread_cursor().thread(), Some(&ids[2]));

    // Esc in the threads pane leaves; the pane stays.
    app.focus_threads_pane();
    app.leave_threads_pane();
    assert_eq!(app.focus(), Focus::View);
    assert!(app.threads_pane_shown(), "Esc leaves, it does not hide");
    Ok(())
}

#[test]
fn the_highlight_prefers_the_thread_starting_under_the_cursor() -> anyhow::Result<()> {
    let dir = fixture("overlap")?;
    let mut app = source_app(&dir)?;
    app.show_tree();
    app.show_threads_pane();
    // A long thread over L3-5, then a short one at L4 inside it.
    app.view_mut().goto_source_line(3);
    app.view_mut().select_lines();
    app.view_mut().move_down(2);
    app.start_comment();
    app.compose_insert("long");
    app.compose_submit();
    annotate(&mut app, 4, "short");
    assert_eq!(app.threads_pane_entries().len(), 2);
    assert_eq!(
        app.threads_pane_entries()[1].range().map(|r| r.start()),
        Some(4)
    );

    // Clicking either row of the second entry highlights it, not the
    // long thread that also covers L4.
    app.threads_pane_click(3);
    assert_eq!(app.threads_pane_selected(), Some(1));
    assert_eq!(app.focus(), Focus::ThreadsPane);
    app.view_mut().goto_source_line(3);
    assert_eq!(app.threads_pane_selected(), Some(0));
    app.threads_pane_click(2);
    assert_eq!(app.threads_pane_selected(), Some(1));
    Ok(())
}

#[test]
fn the_mouse_clicks_wheels_and_focuses_the_pane() -> anyhow::Result<()> {
    let dir = fixture("mouse")?;
    let mut app = source_app(&dir)?;
    app.show_tree();
    app.show_threads_pane();
    annotate(&mut app, 2, "two");
    annotate(&mut app, 6, "six");
    app.view_mut().goto_source_line(1);
    let mouse = |kind, column: usize, row: usize| MouseEvent {
        kind,
        column: u16::try_from(column).unwrap_or(u16::MAX),
        row: u16::try_from(row).unwrap_or(u16::MAX),
        modifiers: KeyModifiers::NONE,
    };
    let down = MouseEventKind::Down(MouseButton::Left);
    let top = app.tree_rows();
    assert_eq!(app.pane_rows() - top, 8);

    // A click on either row of an entry puts the cursor on its thread
    // and the keys with the pane.
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 2, top + 5));
    assert_eq!(app.view().cursor_source_line(), Some(6));
    assert_eq!(app.focus(), Focus::ThreadsPane);
    // A click on the header focuses the pane.
    app.focus_pane(Focus::View);
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 2, top + 1));
    assert_eq!(app.focus(), Focus::ThreadsPane);
    // The wheel steps between threads.
    crate::app::input::mouse::handle_mouse(&mut app, mouse(MouseEventKind::ScrollUp, 2, top + 1));
    assert_eq!(app.view().cursor_source_line(), Some(2));
    // A click on the tree above leaves the pane.
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 2, 1));
    assert_eq!(app.focus(), Focus::Tree);
    Ok(())
}
