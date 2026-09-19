use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fathomable_core::annotations::{
    AgentReplyCommand, Draft, LineRange, MessageTarget, Status, Store, Thread,
};

use fathomable_core::annotations::Author;
use fathomable_core::highlight::Highlighter;

use anyhow::Context as _;

use crate::app::Focus;
use fathomable_testing::TempDir;

use crate::app::testing::{self, app, press};

use fathomable_core::editor::{Cursor, Edit, Motion};

use super::{ComposeTarget, ThreadState};
use crate::app::draw::message::MESSAGE_INDENT;
use crate::app::threads::draft::DraftRow;
use crate::app::threads::list::{BODY_INDENT, ReviewView, Row};
use crate::app::threads::stubs::Subject;
use crate::app::{App, Popup};
use fathomable_core::layout::{Face, Line};

fn type_in(app: &mut App, text: &str) {
    for ch in text.chars() {
        if ch == '\n' {
            app.compose_edit(Edit::Newline);
        } else {
            app.compose_insert(&ch.to_string());
        }
    }
}

#[test]
fn board_history_views_archive_restore_and_recent_resolution() -> anyhow::Result<()> {
    let dir = testing::workspace("board-history-views", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "keep the discussion")?;
    let id = app.marks()[0].id().clone();
    let origin = app
        .thread(&id)
        .ok_or_else(|| anyhow::anyhow!("thread"))?
        .origin();
    assert_eq!(
        origin.side(),
        fathomable_core::annotations::OriginSide::Target
    );
    assert!(
        matches!(
            origin.version(),
            fathomable_core::annotations::OriginVersion::WorkingTree { .. }
        ),
        "working-tree comments retain their displayed endpoint"
    );
    assert!(origin.comparison().is_some());

    app.toggle_resolved(&id);
    let resolution = app
        .thread(&id)
        .and_then(Thread::latest_resolution)
        .ok_or_else(|| anyhow::anyhow!("resolution history"))?;
    let checkout = dir.0.join("ws").canonicalize()?.display().to_string();
    assert_eq!(resolution.checkout(), Some(checkout.as_str()));
    app.open_review_view(ReviewView::RecentlyResolved);
    assert_eq!(app.review_entries(false).len(), 1);
    assert_eq!(app.review_entries(false)[0].id(), &id);

    app.archive_thread(&id);
    assert!(app.review_entries(false).is_empty());
    app.open_review_view(ReviewView::Archived);
    assert_eq!(app.review_entries(false).len(), 1);
    assert!(app.thread(&id).is_some_and(Thread::is_archived));

    app.restore_thread(&id);
    assert!(app.review_entries(false).is_empty());
    assert!(!app.thread(&id).is_some_and(Thread::is_archived));
    assert_eq!(
        app.thread(&id).map(Thread::status),
        Some(Status::Resolved),
        "restore retains lifecycle"
    );
    app.open(Path::new("README.md"));
    assert!(!app.review_list().is_open());
    assert_eq!(app.review().view, ReviewView::Board);
    Ok(())
}

fn three_resolved_threads(
    dir: &TempDir,
    archive: bool,
) -> anyhow::Result<Vec<fathomable_core::annotations::ThreadId>> {
    let mut store = Store::open(testing::store_path(dir))?;
    let mut ids = Vec::new();
    for (offset, comment) in ["A", "B", "C"].into_iter().enumerate() {
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("README.md"),
                LineRange::new(offset + 1, offset + 1),
                comment,
            ),
            testing::README,
            u64::try_from(offset + 1)?,
        )?;
        store.resolve(&id, None, u64::try_from(offset + 10)?)?;
        if archive {
            store.archive(&id, u64::try_from(offset + 20)?)?;
        }
        ids.push(id);
    }
    Ok(ids)
}

#[test]
fn archive_and_restore_reseat_middle_sidebar_entry_before_open_reviews() -> anyhow::Result<()> {
    let restore_dir = testing::workspace("sidebar-reseat-restore", testing::README)?;
    let restore_ids = three_resolved_threads(&restore_dir, true)?;
    let mut restore_app = testing::source_app(&restore_dir)?;
    restore_app.open_review_view(ReviewView::Archived);
    restore_app.show_threads_pane();
    restore_app.window_threads();
    restore_app.set_thread_cursor(restore_ids[1].clone());
    assert_eq!(restore_app.review_selected_index(), Some(1));
    assert_eq!(restore_app.threads_pane_selected(), Some(1));

    restore_app.restore_thread(&restore_ids[1]);
    assert!(restore_app.review_list().is_open());
    assert_eq!(restore_app.focus(), Focus::ThreadsPane);
    assert_eq!(
        restore_app.thread_cursor().thread(),
        Some(&restore_ids[2]),
        "restoring the middle archived entry reseats to its next neighbor"
    );
    assert_eq!(restore_app.review_selected_index(), Some(1));
    assert_eq!(restore_app.threads_pane_selected(), Some(1));

    let archive_dir = testing::workspace("sidebar-reseat-archive", testing::README)?;
    let archive_ids = three_resolved_threads(&archive_dir, false)?;
    let mut archive_app = testing::source_app(&archive_dir)?;
    archive_app.open_review_view(ReviewView::Board);
    archive_app.review_toggle_resolved();
    archive_app.show_threads_pane();
    archive_app.window_threads();
    archive_app.set_thread_cursor(archive_ids[1].clone());
    assert_eq!(archive_app.review_selected_index(), Some(1));
    assert_eq!(archive_app.threads_pane_selected(), Some(1));

    archive_app.archive_thread(&archive_ids[1]);
    assert!(archive_app.review_list().is_open());
    assert_eq!(archive_app.focus(), Focus::ThreadsPane);
    assert_eq!(
        archive_app.thread_cursor().thread(),
        Some(&archive_ids[2]),
        "archiving the middle resolved entry reseats to its next neighbor"
    );
    assert_eq!(archive_app.review_selected_index(), Some(1));
    assert_eq!(archive_app.threads_pane_selected(), Some(1));
    Ok(())
}

#[test]
fn clear_board_confirmation_cancels_without_archiving() -> anyhow::Result<()> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let dir = testing::workspace("clear-board-confirmation", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "keep the board")?;
    let id = app.marks()[0].id().clone();

    app.request_clear_board();
    assert!(matches!(app.popup(), Some(Popup::ConfirmBoard { .. })));
    assert_eq!(
        crate::app::input::keys::handle_key(&mut app, testing::key('y')),
        crate::app::view::Effect::None
    );
    assert!(matches!(app.popup(), Some(Popup::ConfirmBoard { .. })));
    crate::app::input::keys::handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.thread(&id).is_some_and(Thread::is_archived));

    app.request_clear_board();
    crate::app::input::keys::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );
    assert!(app.thread(&id).is_some_and(Thread::is_archived));
    Ok(())
}

#[test]
fn clear_board_confirmation_mouse_controls_and_outside_cancel() -> anyhow::Result<()> {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    let dir = testing::workspace("clear-board-confirmation-mouse", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "keep the board")?;
    let id = app.marks()[0].id().clone();
    let mouse = |kind, column: usize, row: usize| MouseEvent {
        kind,
        column: u16::try_from(column).unwrap_or(u16::MAX),
        row: u16::try_from(row).unwrap_or(u16::MAX),
        modifiers: KeyModifiers::NONE,
    };
    let down = MouseEventKind::Down(MouseButton::Left);

    app.request_clear_board();
    let layout = crate::app::draw::board_confirmation_layout(&app, false);
    let cancel = (usize::from(layout.popup.x)..usize::from(layout.popup.right()))
        .find_map(|column| {
            (usize::from(layout.popup.y)..usize::from(layout.popup.bottom()))
                .find(|&row| {
                    layout.action_at(column, row)
                        == Some(crate::app::input::bindings::Action::Escape)
                })
                .map(|row| (column, row))
        })
        .ok_or_else(|| anyhow::anyhow!("visible cancel control"))?;
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, cancel.0, cancel.1));
    assert!(app.popup().is_none());
    assert!(!app.thread(&id).is_some_and(Thread::is_archived));

    app.request_clear_board();
    let layout = crate::app::draw::board_confirmation_layout(&app, false);
    let passive = (
        usize::from(layout.popup.x) + 1,
        usize::from(layout.popup.y) + 1,
    );
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, passive.0, passive.1));
    assert!(matches!(app.popup(), Some(Popup::ConfirmBoard { .. })));

    app.window_files();
    assert_eq!(app.focus(), Focus::Tree);
    let outside = (
        usize::from(layout.popup.right()),
        usize::from(layout.popup.y),
    );
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, outside.0, outside.1));
    assert!(app.popup().is_none());
    assert!(!app.thread(&id).is_some_and(Thread::is_archived));
    assert_eq!(app.focus(), Focus::Tree, "outside click is consumed");

    app.request_clear_board();
    let layout = crate::app::draw::board_confirmation_layout(&app, false);
    let confirm = (usize::from(layout.popup.x)..usize::from(layout.popup.right()))
        .find_map(|column| {
            (usize::from(layout.popup.y)..usize::from(layout.popup.bottom()))
                .find(|&row| {
                    layout.action_at(column, row)
                        == Some(crate::app::input::bindings::Action::Confirm)
                })
                .map(|row| (column, row))
        })
        .ok_or_else(|| anyhow::anyhow!("visible clear control"))?;
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, confirm.0, confirm.1));
    assert!(app.thread(&id).is_some_and(Thread::is_archived));
    Ok(())
}

#[test]
fn clear_board_uses_the_acknowledged_slate_and_refreshes_changed_counts() -> anyhow::Result<()> {
    let dir = testing::workspace("clear-board-race", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "acknowledged")?;
    let acknowledged = app.marks()[0].id().clone();

    app.request_clear_board();
    let mut writer = Store::open(testing::store_path(&dir))?;
    let later = writer.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(4, 4),
            "arrived later",
        ),
        testing::README,
        20,
    )?;
    app.confirm_clear_board();
    assert!(app.thread(&acknowledged).is_some_and(Thread::is_archived));
    assert!(!app.thread(&later).is_some_and(Thread::is_archived));

    app.restore_thread(&acknowledged);
    app.request_clear_board();
    Store::open(testing::store_path(&dir))?.reply_user(
        &acknowledged,
        21,
        "changed after confirmation",
        fathomable_core::annotations::UserSubmit::Normal,
    )?;
    app.confirm_clear_board();
    assert!(matches!(
        app.popup(),
        Some(Popup::ConfirmBoard { changed: true, .. })
    ));
    assert!(!app.thread(&acknowledged).is_some_and(Thread::is_archived));
    Ok(())
}

/// Annotate L3-5 of the open README with `comment`.
fn annotate(app: &mut App, comment: &str) -> anyhow::Result<()> {
    app.view_mut().move_down(2);
    app.view_mut().select_lines();
    press(app, "c");
    type_in(app, comment);
    app.compose_submit();
    anyhow::ensure!(app.thread_counts().1 == 1, "thread not created");
    Ok(())
}

fn app_with_review_messages(
    name: &str,
) -> anyhow::Result<(TempDir, App, fathomable_core::annotations::ThreadId)> {
    let dir = testing::workspace(&format!("threads-{name}"), testing::README)?;
    let mut app = app(&dir)?;
    let opening = (1..=30)
        .map(|line| format!("opening line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    annotate(&mut app, &opening)?;
    let id = app.marks()[0].id().clone();
    testing::external_agent_reply(
        &mut app,
        &id,
        Author::agent("reviewer"),
        "agent answer",
        false,
        None,
    )?;
    app.expand_thread(id.clone());
    app.thread_reply();
    type_in(&mut app, "user follow-up");
    app.compose_submit();
    app.view_mut().goto_bottom();
    app.start_comment();
    type_in(&mut app, "bottom");
    app.compose_submit();
    app.view_mut().goto_top();
    app.open_review();
    Ok((dir, app, id))
}

#[test]
fn a_rename_carries_the_threads_and_the_open_document() -> anyhow::Result<()> {
    use crate::app::watch::Event;
    let dir = testing::workspace("threads-rename", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "keep me")?;
    let id = app.marks()[0].id().clone();
    app.view_mut().move_down(1);
    let cursor = app.view().cursor();

    // A file rename: this checkout's view follows with cursor and marks
    // intact, while the shared store keeps the repository origin path.
    fs::create_dir_all(dir.0.join("ws/docs"))?;
    fs::rename(dir.0.join("ws/README.md"), dir.0.join("ws/docs/GUIDE.md"))?;
    app.on_events(vec![Event::Renamed {
        from: dir.0.join("ws/README.md"),
        to: dir.0.join("ws/docs/GUIDE.md"),
    }]);
    app.settle_background();
    assert_eq!(app.current_path(), Path::new("docs/GUIDE.md"));
    assert_eq!(app.message(), Some("renamed to docs/GUIDE.md"));
    assert_eq!(app.view().cursor(), cursor);
    assert_eq!(app.marks()[0].range(), Some(LineRange::new(3, 5)));
    assert_eq!(app.mark_in(LineRange::new(4, 4)), Some(ThreadState::Active));
    assert_eq!(
        app.thread(&id).map(|thread| app.thread_path(thread)),
        Some(Path::new("docs/GUIDE.md"))
    );
    let store = Store::open(dir.0.join("state/threads.jsonl"))?;
    assert_eq!(
        store.thread(&id).map(Thread::path),
        Some(Path::new("README.md")),
        "one checkout does not rewrite the shared board path"
    );

    // A directory rename moves everything under it by prefix.
    fs::rename(dir.0.join("ws/docs"), dir.0.join("ws/notes"))?;
    app.on_events(vec![Event::Renamed {
        from: dir.0.join("ws/docs"),
        to: dir.0.join("ws/notes"),
    }]);
    app.settle_background();
    assert_eq!(app.current_path(), Path::new("notes/GUIDE.md"));
    assert_eq!(
        app.thread(&id).map(|thread| app.thread_path(thread)),
        Some(Path::new("notes/GUIDE.md"))
    );
    assert_eq!(app.thread_counts(), (1, 1));

    // A later edit reloads from the new path.
    fs::write(
        dir.0.join("ws/notes/GUIDE.md"),
        "# Readme\n\nintro\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
    )?;
    app.on_events(vec![Event::Change(dir.0.join("ws/notes/GUIDE.md"))]);
    app.settle_background();
    assert!(app.view().text().contains("intro"));
    assert_eq!(app.marks()[0].range(), Some(LineRange::new(5, 7)));
    Ok(())
}

#[test]
fn a_deleted_file_keeps_its_content_and_refuses_new_comments() -> anyhow::Result<()> {
    use crate::app::watch::Event;
    let dir = testing::workspace("threads-deleted", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "still here")?;
    let id = app.marks()[0].id().clone();
    fs::write(dir.0.join("ws/other.md"), "# Other\n")?;

    fs::remove_file(dir.0.join("ws/README.md"))?;
    app.on_events(vec![Event::Removed(dir.0.join("ws/README.md"))]);
    app.settle_background();
    assert!(app.deleted());
    assert_eq!(
        app.banner(),
        Some("deleted from worktree · showing last loaded")
    );
    assert!(app.view().text().contains("alpha"), "last content stays");
    assert_eq!(app.thread_counts(), (1, 1), "threads still read");
    assert!(
        app.status_lines()
            .iter()
            .any(|(_, v)| v.contains("deleted"))
    );
    app.start_new_comment();
    assert!(app.popup().is_none());
    assert!(app.message().is_some_and(|m| m.contains("deleted")));
    app.expand_thread(id.clone());
    app.thread_reply();
    assert!(!matches!(app.popup(), Some(Popup::Compose(_))));

    // Shown again while still gone: the retained source remains searchable.
    app.open(Path::new("other.md"));
    assert!(app.info().is_none());
    app.open(Path::new("README.md"));
    assert!(app.info().is_none());
    assert!(app.view().text().contains("alpha"));
    assert_eq!(
        app.banner(),
        Some("deleted from worktree · showing last loaded")
    );

    // Back on disk: reloaded, banner gone, thread re-anchored.
    fs::write(
        dir.0.join("ws/README.md"),
        "# Readme\n\nintro\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
    )?;
    app.on_events(vec![Event::Created(dir.0.join("ws/README.md"))]);
    app.settle_background();
    assert!(!app.deleted());
    assert!(app.info().is_none());
    assert!(app.view().text().contains("intro"));
    assert_eq!(app.marks()[0].range(), Some(LineRange::new(5, 7)));
    app.start_new_comment();
    assert!(matches!(app.popup(), Some(Popup::Compose(_))));
    Ok(())
}

#[test]
fn changing_a_tombstones_git_source_relocates_its_threads() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-tombstone-source", testing::README)?;
    let root = testing::root(&dir);
    fathomable_testing::git::init(&root)?;
    fathomable_testing::git::commit_and_stage(&root, &[("README.md", testing::README)])?;

    let mut original = app(&dir)?;
    annotate(&mut original, "tracks alpha through gamma")?;
    drop(original);

    let index_text = testing::README.replace("\nalpha", "\ninserted\nalpha");
    fathomable_testing::git::stage(&root, &[("README.md", &index_text)])?;
    fs::remove_file(root.join("README.md"))?;
    let mut tombstone = testing::AppBuilder::new(&dir).unopened().build()?;
    tombstone.open(Path::new("README.md"));
    assert_eq!(tombstone.marks()[0].range(), Some(LineRange::new(3, 5)));

    fathomable_testing::git::stage(&root, &[])?;
    tombstone.on_events(vec![crate::app::watch::Event::Change(
        root.join(".git/index"),
    )]);
    tombstone.settle_status();
    assert_eq!(tombstone.view().text(), testing::README);
    assert_eq!(tombstone.marks()[0].range(), Some(LineRange::new(3, 5)));
    Ok(())
}

#[test]
fn selection_becomes_a_thread_and_survives_reload() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-annotate", testing::README)?;
    let mut app = app(&dir)?;
    assert_eq!(app.thread_counts(), (0, 0));
    // Rows: 0 "# Readme", 1 blank, 2 "alpha beta gamma" (one paragraph).
    app.view_mut().move_down(2);
    app.view_mut().select_lines();
    app.start_comment();
    let Some(Popup::Compose(compose)) = app.popup() else {
        anyhow::bail!("the draft did not open");
    };
    assert_eq!(compose.target(), &ComposeTarget::New(LineRange::new(3, 5)));
    type_in(&mut app, "tighten\nthis");
    app.compose_submit();
    assert!(app.popup().is_none());
    assert_eq!(app.message(), Some("commented on L3-5"));
    assert_eq!(app.thread_counts(), (1, 1));
    assert_eq!(app.mark_in(LineRange::new(4, 4)), Some(ThreadState::Active));
    assert_eq!(app.mark_in(LineRange::new(1, 1)), None);
    assert!(
        app.view().selection().is_none(),
        "selection cleared after commenting"
    );

    // Insert lines above: the mark follows the content.
    fs::write(
        dir.0.join("ws/README.md"),
        "# Readme\n\nnew intro\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
    )?;
    app.on_changes(vec![dir.0.join("ws/README.md")]);
    assert_eq!(app.marks()[0].range(), Some(LineRange::new(5, 7)));
    assert_eq!(app.mark_in(LineRange::new(3, 3)), None);

    // Edit one of them: the thread follows onto the rewritten lines
    // and reads as edited, on disk too (ADR 0019).
    fs::write(
        dir.0.join("ws/README.md"),
        "# Readme\n\nnew intro\n\nalpha\nBETA\ngamma\n\n- one\n- two\n",
    )?;
    app.on_changes(vec![dir.0.join("ws/README.md")]);
    assert_eq!(app.marks()[0].range(), Some(LineRange::new(5, 7)));
    assert!(app.marks()[0].placement().is_edited());
    assert_eq!(app.mark_in(LineRange::new(6, 6)), Some(ThreadState::Active));
    let reopened = Store::open(dir.0.join("state/threads.jsonl"))?;
    assert!(reopened.threads()[0].reanchored_at().is_some());
    assert_eq!(reopened.threads()[0].range(), Some(LineRange::new(5, 7)));

    // The user's reply acknowledges the edit.
    app.view_mut().move_down(3);
    app.expand_at_cursor();
    app.thread_reply();
    type_in(&mut app, "still fine");
    app.compose_submit();
    assert_eq!(app.mark_in(LineRange::new(6, 6)), Some(ThreadState::Active));

    // Rewrite everything: the thread detaches at its last known range.
    fs::write(dir.0.join("ws/README.md"), "# Readme\n\ngone\n")?;
    app.on_changes(vec![dir.0.join("ws/README.md")]);
    assert!(app.marks()[0].is_detached());
    assert_eq!(app.mark_in(LineRange::new(5, 5)), None);
    // Detachment is a location suffix, never a lifecycle glyph.
    let rows = app.threads_pane_entries();
    assert_eq!(rows[0].place(), "L5-7?");
    assert_eq!(rows[0].words().state(), ThreadState::Active);
    assert_eq!(rows[0].words().glyph(), "●");
    Ok(())
}

#[test]
fn an_expanded_thread_renders_header_authors_and_badge() -> anyhow::Result<()> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let dir = testing::workspace("threads-render", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(80, 24);
    app.view_mut().move_down(2);
    app.view_mut().select_lines();
    app.start_comment();
    type_in(&mut app, "What is this?");
    app.compose_submit();
    let id = app.marks()[0].id().clone();
    let long = (1..=12)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    testing::external_agent_reply(
        &mut app,
        &id,
        Author::Agent {
            name: "Copilot".to_owned(),
            client: Some("github-copilot-developer".to_owned()),
            id: None,
        },
        &long,
        true,
        None,
    )?;
    app.expand_thread(id);

    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    let render = |app: &App| -> anyhow::Result<Vec<String>> {
        let mut terminal = Terminal::new(TestBackend::new(80, 36))?;
        terminal.draw(|frame| crate::app::draw::draw(frame, app, &theme))?;
        let buffer = terminal.backend().buffer().clone();
        Ok((0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect())
    };
    app.resize(80, 36);
    // Every message shows, under a header with the state; the keys are
    // on the bar (ADR 0067); there is no END row and nothing to scroll
    // (ADR 0049).
    let rows = render(&app)?;
    let screen = rows.join("\n");
    assert!(
        rows.iter()
            .any(|row| row.contains("◐ ▾") && row.contains("resolve proposed")),
        "header carries dim lifecycle status:\n{screen}"
    );
    assert!(
        rows[app.text_bar_row()].contains("auto-resolve R"),
        "the bar carries the keys:\n{screen}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("Copilot") && row.contains("[proposes resolving]")),
        "short author and badge share the row:\n{screen}"
    );
    assert!(
        !screen.contains("github-copilot-developer"),
        "client id is not shown"
    );
    assert!(
        rows.iter().any(|row| row.contains("line 12")),
        "the whole reply shows:\n{screen}"
    );
    assert!(
        !screen.contains("─── END ───") && !rows.iter().any(|row| row.contains("▼")),
        "no END row and no overflow:\n{screen}"
    );

    // A reply is written under the last message: the author row with
    // the draft keys, then the text indented as a body is (ADR 0054).
    app.thread_reply();
    type_in(&mut app, "in the thread");
    let rows = render(&app)?;
    let screen = rows.join("\n");
    let last = rows
        .iter()
        .position(|row| row.contains("line 12"))
        .context("the reply's last row")?;
    assert!(
        rows[last + 1].contains(" User  draft") && !rows[last + 1].contains("submit"),
        "the author row follows the last message, its keys on the bar (ADR 0067):\n{screen}"
    );
    assert!(
        rows[app.text_bar_row()].contains("submit Enter"),
        "the bar carries the draft's keys:\n{screen}"
    );
    assert!(
        rows[last + 2].contains("   in the thread"),
        "the draft's text follows the author row:\n{screen}"
    );
    assert!(
        !screen.contains("───") && !screen.contains("reply on L"),
        "no box and no rule along the bottom:\n{screen}"
    );
    app.compose_cancel();
    Ok(())
}

#[test]
fn an_expanded_thread_replies_resolves_and_reopens() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-panel", testing::README)?;
    let mut app = app(&dir)?;
    app.thread_reply();
    assert_eq!(app.message(), Some("no thread here"));
    app.start_comment();
    type_in(
        &mut app,
        &(1..=20)
            .map(|n| format!("first {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    app.compose_submit();
    assert_eq!(app.mark_in(LineRange::new(1, 1)), Some(ThreadState::Active));

    app.expand_at_cursor();
    anyhow::ensure!(app.shows_thread(), "the thread did not expand");
    let id = app.thread_cursor().thread().cloned().context("no cursor")?;
    assert_eq!(app.thread_position(), Some((1, 1)));
    assert_eq!(app.focus(), Focus::View);
    app.thread_reply();
    assert!(
        matches!(app.popup(), Some(Popup::Compose(c)) if c.target() == &ComposeTarget::Reply(id.clone()))
    );
    assert!(
        app.shows_thread(),
        "the thread stays readable while replying"
    );
    app.resize(100, 16);
    app.compose_scroll(2);
    assert_eq!(app.view().scroll(), 2, "Alt-Down scrolls the text behind");
    app.compose_cancel();
    assert!(app.popup().is_none());
    assert_eq!(app.focus(), Focus::View);
    app.thread_reply();
    type_in(&mut app, "second thoughts");
    app.compose_submit();
    assert!(app.popup().is_none());
    assert!(
        app.shows_thread(),
        "the thread stays expanded after a reply"
    );
    assert_eq!(app.focus(), Focus::View);
    let thread = app.thread(&id).cloned();
    assert_eq!(thread.as_ref().map(|t| t.replies().len()), Some(1));

    app.thread_toggle_resolved();
    assert_eq!(app.thread(&id).map(Thread::status), Some(Status::Resolved));
    assert_eq!(
        app.mark_in(LineRange::new(1, 1)),
        Some(ThreadState::Resolved)
    );
    assert_eq!(app.thread_counts(), (0, 1));
    app.thread_toggle_resolved();
    assert_eq!(app.thread(&id).map(Thread::status), Some(Status::Open));

    // Everything is on disk for the next session.
    let again = Store::open(dir.0.join("state/threads.jsonl"))?;
    assert_eq!(again.threads().len(), 1);
    assert_eq!(again.threads()[0].replies()[0].body(), "second thoughts");
    Ok(())
}

#[test]
fn thread_keys_select_messages_and_edit_only_the_users() -> anyhow::Result<()> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::app::input::keys;

    let dir = testing::workspace("threads-message-nav", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "opening")?;
    let id = app.marks()[0].id().clone();
    testing::external_agent_reply(
        &mut app,
        &id,
        Author::agent("reviewer"),
        "agent answer",
        false,
        None,
    )?;
    app.goto_message(id.clone(), 1);
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    assert_eq!(
        Some(app.thread_cursor().message()),
        Some(1),
        "the newest message starts selected"
    );
    keys::handle_key(&mut app, key(KeyCode::Char('e')));
    assert!(app.popup().is_none());
    assert_eq!(app.message(), Some("only your messages can be edited"));

    keys::handle_key(&mut app, key(KeyCode::Char('h')));
    assert_eq!(Some(app.thread_cursor().message()), Some(0));
    keys::handle_key(&mut app, key(KeyCode::Char('e')));
    assert!(matches!(
        app.popup(),
        Some(Popup::Compose(compose))
            if compose.target()
                == &ComposeTarget::Edit {
                    thread: id.clone(),
                    message: MessageTarget::Comment,
                }
    ));
    assert_eq!(app.compose_draft(), Some("opening"));
    app.set_compose_text("revised opening");
    app.compose_submit();
    assert_eq!(
        app.thread(&id).map(Thread::comment),
        Some("revised opening")
    );

    app.thread_reply();
    type_in(&mut app, "user follow-up");
    app.compose_submit();
    assert_eq!(Some(app.thread_cursor().message()), Some(2));
    keys::handle_key(&mut app, key(KeyCode::Char('e')));
    assert!(matches!(
        app.popup(),
        Some(Popup::Compose(compose))
            if compose.target()
                == &ComposeTarget::Edit {
                    thread: id.clone(),
                    message: MessageTarget::Reply(1),
                }
    ));
    assert_eq!(app.compose_draft(), Some("user follow-up"));
    app.compose_cancel();
    assert!(app.popup().is_none(), "an unchanged edit closes at once");
    keys::handle_key(&mut app, key(KeyCode::Char('h')));
    assert_eq!(app.thread_cursor().message(), 1);
    keys::handle_key(&mut app, key(KeyCode::Char('h')));
    assert_eq!(app.thread_cursor().message(), 0, "h reaches the comment");
    keys::handle_key(&mut app, key(KeyCode::Char('G')));
    assert_eq!(app.thread_cursor().message(), 2, "G is the newest");
    Ok(())
}

#[test]
fn review_keys_select_messages_and_edit_only_the_users() -> anyhow::Result<()> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::app::input::keys;

    let (_dir, mut app, id) = app_with_review_messages("list-message-nav")?;
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    assert_eq!(
        app.thread_cursor().message(),
        2,
        "the newest message starts selected"
    );
    let rows = app.review_rows(60);
    let selected_row = rows
        .rows
        .iter()
        .position(|row| matches!(row, Row::Message { selected: true, .. }))
        .context("no selected message row")?;
    let visible = app.text_rows().saturating_sub(1).max(1);
    assert!(
        (app.review_list().scroll()..app.review_list().scroll() + visible).contains(&selected_row),
        "the selected message is visible"
    );

    keys::handle_key(&mut app, key(KeyCode::Char('h')));
    assert_eq!(app.thread_cursor().message(), 1);
    keys::handle_key(&mut app, key(KeyCode::Char('e')));
    assert!(app.popup().is_none());
    assert_eq!(app.message(), Some("only your messages can be edited"));

    keys::handle_key(&mut app, key(KeyCode::Char('h')));
    assert_eq!(app.thread_cursor().message(), 0);
    keys::handle_key(&mut app, key(KeyCode::Char('e')));
    assert!(matches!(
        app.popup(),
        Some(Popup::Compose(compose))
            if compose.target()
                == &ComposeTarget::Edit {
                    thread: id.clone(),
                    message: MessageTarget::Comment,
                }
    ));
    // The list gave the column to the file, where the draft stands in
    // for the comment's 31 rows and the replies keep theirs (ADR 0054).
    assert!(!app.review_list().is_open(), "the list closed to write");
    assert!(app.shows_thread(), "the thread is expanded in the text");
    let stub = app.stubs().iter().next().context("the thread's block")?;
    assert_eq!(stub.draft_slot(), Some((1, 31)));
    assert_eq!(app.draft_rows(), 31, "the author row and thirty lines");
    assert_eq!(
        stub.row_of_message(1),
        Some(32),
        "the agent's reply follows the draft"
    );
    app.set_compose_text("revised opening");
    assert_eq!(app.draft_rows(), 2);
    assert_eq!(
        app.stubs()[0].row_of_message(1),
        Some(3),
        "the replies move up with the shorter draft"
    );
    app.compose_submit();
    assert_eq!(
        app.focus(),
        Focus::Review,
        "the list comes back with the keys"
    );
    assert!(app.review_list().is_open());
    assert_eq!(
        app.thread(&id).map(Thread::comment),
        Some("revised opening")
    );

    keys::handle_key(&mut app, key(KeyCode::Char('j')));
    assert_eq!(app.thread_cursor().message(), 0);
    keys::handle_key(&mut app, key(KeyCode::Char('k')));
    assert_eq!(
        app.thread_cursor().message(),
        2,
        "changing threads selects the newest message"
    );
    keys::handle_key(&mut app, key(KeyCode::Char('e')));
    assert!(matches!(
        app.popup(),
        Some(Popup::Compose(compose))
            if compose.target()
                == &ComposeTarget::Edit {
                    thread: id.clone(),
                    message: MessageTarget::Reply(1),
                }
    ));
    assert_eq!(app.compose_draft(), Some("user follow-up"));
    app.compose_cancel();
    assert_eq!(app.focus(), Focus::Review);
    Ok(())
}

#[test]
fn review_mouse_selects_messages_and_reply_selects_itself() -> anyhow::Result<()> {
    let (_dir, mut app, id) = app_with_review_messages("list-message-mouse")?;
    let rows = app.review_rows(100);
    let agent_row = rows
        .rows
        .iter()
        .position(|row| {
            matches!(
                row,
                Row::Message {
                    entry: 0,
                    message: 1,
                    ..
                }
            )
        })
        .context("no agent message row")?;
    let scroll = app.review_list().scroll();
    anyhow::ensure!(agent_row >= scroll, "agent message is above the viewport");
    app.review_click(agent_row - scroll);
    assert_eq!(
        app.thread_cursor().message(),
        1,
        "a click selects its message"
    );
    app.thread_reply();
    type_in(&mut app, "reply from the list");
    app.compose_submit();
    assert_eq!(
        app.thread_cursor().message(),
        3,
        "a reply sent from the list becomes selected"
    );
    app.thread_edit_message();
    assert!(matches!(
        app.popup(),
        Some(Popup::Compose(compose))
            if compose.target()
                == &ComposeTarget::Edit {
                    thread: id.clone(),
                    message: MessageTarget::Reply(2),
                }
    ));
    assert_eq!(app.compose_draft(), Some("reply from the list"));
    app.compose_cancel();

    let rows = app.review_rows(100);
    let agent_row = rows
        .rows
        .iter()
        .position(|row| {
            matches!(
                row,
                Row::Message {
                    entry: 0,
                    message: 1,
                    ..
                }
            )
        })
        .context("no agent message row after reply")?;
    app.review_click(agent_row - app.review_list().scroll());
    app.thread_open_in_file();
    assert_eq!(
        Some(app.thread_cursor().message()),
        Some(1),
        "Enter carries the selected message into the pane"
    );
    Ok(())
}

#[test]
fn mouse_targets_the_pane_under_the_pointer() -> anyhow::Result<()> {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use crate::app::Border;

    let mouse = |kind, column: usize, row: usize| MouseEvent {
        kind,
        column: u16::try_from(column).unwrap_or(u16::MAX),
        row: u16::try_from(row).unwrap_or(u16::MAX),
        modifiers: KeyModifiers::NONE,
    };
    let down = MouseEventKind::Down(MouseButton::Left);
    let drag = MouseEventKind::Drag(MouseButton::Left);
    let up = MouseEventKind::Up(MouseButton::Left);

    let dir = testing::workspace("threads-mouse", testing::README)?;
    let mut app = app(&dir)?;
    app.start_comment();
    let long = (1..=20)
        .map(|n| format!("row {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    type_in(&mut app, &long);
    app.compose_submit();
    app.expand_at_cursor();
    assert_eq!(app.focus(), Focus::View);
    app.resize(100, 20);

    // The wheel scrolls the text under the pointer, focus aside.
    crate::app::input::mouse::handle_mouse(&mut app, mouse(MouseEventKind::ScrollDown, 20, 2));
    assert_eq!(app.view().scroll(), 3);
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 20, 0));
    assert_eq!(app.focus(), Focus::View, "a click on the text focuses it");
    assert!(app.shows_thread(), "the thread stays expanded");
    crate::app::input::mouse::handle_mouse(&mut app, mouse(MouseEventKind::ScrollUp, 20, 2));
    assert_eq!(app.view().scroll(), 0);

    // Dragging the tree's divider resizes the tree.
    app.toggle_tree_focus();
    let width = app.sidebar_width();
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, width - 1, 3));
    assert_eq!(app.dragging(), Some(Border::Sidebar));
    crate::app::input::mouse::handle_mouse(&mut app, mouse(drag, 44, 3));
    assert_eq!(app.sidebar_width(), 45);
    crate::app::input::mouse::handle_mouse(&mut app, mouse(drag, 2, 3));
    assert_eq!(app.sidebar_width(), 8, "no narrower than the minimum");
    crate::app::input::mouse::handle_mouse(&mut app, mouse(up, 2, 3));
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 3, 0));
    assert_eq!(app.focus(), Focus::Tree, "the header row focuses the tree");

    // The draft keeps the keys but lets the mouse through; opening it
    // scrolled the view to show its rows at the thread's end (ADR 0054).
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 20, 0));
    app.thread_reply();
    assert!(matches!(app.popup(), Some(Popup::Compose(_))));
    let (row, _) = app.draft_cursor_cell().context("the draft's cursor")?;
    let shown = app.view().scroll();
    // Above the key bar on the bottom text row (ADR 0067).
    assert!(shown > 0 && row + 1 < shown + app.text_rows(), "revealed");
    crate::app::input::mouse::handle_mouse(&mut app, mouse(MouseEventKind::ScrollDown, 20, 2));
    assert_eq!(
        app.view().scroll(),
        shown + 3,
        "the wheel scrolls the text under the draft"
    );
    crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 20, 0));
    assert!(
        matches!(app.popup(), Some(Popup::Compose(_))),
        "a click away leaves the draft open"
    );
    Ok(())
}

#[test]
fn the_status_line_badges_do_not_depend_on_focus() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-status", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "one")?;
    app.view_mut().toggle_source_view();
    let parts = crate::app::draw::status_parts(&app);
    assert_eq!(parts.pill, "FILE");
    assert_eq!(parts.badges, ["CMP empty tree → working tree"]);
    assert!(
        parts.right_text().contains("1 threads"),
        "{}",
        parts.right_text()
    );
    app.view_mut().select_lines();
    assert_eq!(
        crate::app::draw::status_parts(&app).pill,
        "FILE",
        "selection does not rename the File surface"
    );
    app.focus_threads_pane();
    let parts = crate::app::draw::status_parts(&app);
    assert_eq!(parts.pill, "THREAD LIST");
    assert_eq!(
        parts.badges,
        ["CMP empty tree → working tree"],
        "the badges outlive the focus change"
    );
    Ok(())
}

/// Esc returns from a list to the displayed main view, `T` focuses the
/// Thread list, `w` cycles back, and only `f` leaves Threads.
#[test]
fn esc_returns_to_the_displayed_main_without_closing_threads() -> anyhow::Result<()> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::app::input::keys;

    let dir = testing::workspace("threads-toggle", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "one")?;
    let press = |app: &mut App, code| {
        keys::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
    };
    let space = |app: &mut App, ch| {
        press(app, KeyCode::Char(' '));
        press(app, KeyCode::Char(ch));
    };

    app.focus_threads_pane();
    assert_eq!(app.focus(), Focus::ThreadsPane);
    press(&mut app, KeyCode::Esc);
    assert_eq!(app.focus(), Focus::View);
    assert!(app.threads_pane_shown(), "Esc leaves the pane open");
    press(&mut app, KeyCode::Char('T'));
    assert_eq!(
        app.focus(),
        Focus::ThreadsPane,
        "T lands on the list while it is hidden"
    );
    press(&mut app, KeyCode::Char('w'));
    assert_eq!(app.focus(), Focus::View, "w cycles back");
    assert!(app.threads_pane_shown());
    space(&mut app, 'p');
    press(&mut app, KeyCode::Char('t'));
    assert!(!app.threads_pane_shown(), "Space p t hides it");

    press(&mut app, KeyCode::Char('t'));
    assert!(app.review_list().is_open());
    assert_eq!(app.focus(), Focus::Review);
    app.window_files();
    assert!(app.review_list().is_open());
    assert_eq!(app.focus(), Focus::Tree);
    press(&mut app, KeyCode::Char('t'));
    assert!(app.review_list().is_open(), "t never closes Threads");
    assert_eq!(app.focus(), Focus::Review, "t focuses an open Threads view");
    press(&mut app, KeyCode::Esc);
    assert!(app.review_list().is_open(), "Esc keeps Threads displayed");
    assert_eq!(app.focus(), Focus::Review);
    press(&mut app, KeyCode::Char('f'));
    assert!(!app.review_list().is_open());
    assert_eq!(app.focus(), Focus::View);
    Ok(())
}

#[test]
fn annotation_jumps_wrap_and_picker_lists_threads() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-jumps", testing::README)?;
    let mut app = app(&dir)?;
    app.thread_step_in_file(1);
    assert_eq!(app.message(), Some("no threads in this file"));
    app.start_comment();
    type_in(&mut app, "top");
    app.compose_submit();
    app.view_mut().goto_bottom();
    app.start_comment();
    type_in(&mut app, "bottom");
    app.compose_submit();
    let bottom = app.view().cursor_source_line();
    app.view_mut().goto_top();
    app.thread_step_in_file(1);
    assert_eq!(app.view().cursor_source_line(), bottom);
    app.thread_step_in_file(1);
    assert_eq!(app.view().cursor_source_line(), Some(1));
    assert_eq!(app.message(), Some("wrapped to first thread"));
    app.thread_step_in_file(-1);
    assert_eq!(app.view().cursor_source_line(), bottom);
    app.start_new_comment();
    app.compose_submit();
    assert_eq!(app.message(), Some("empty comment discarded"));
    Ok(())
}

/// `c` starts another thread on an annotated row without changing the
/// existing thread's fold, and thread motions walk the file in line order.
#[test]
fn c_starts_another_thread_and_n_walks_the_file() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-walk", testing::README)?;
    let mut app = app(&dir)?;
    app.view_mut().goto_bottom();
    app.start_comment();
    type_in(&mut app, "bottom");
    app.compose_submit();
    let bottom = app.view().cursor_source_line();
    app.view_mut().goto_top();
    app.start_comment();
    type_in(&mut app, "top");
    app.compose_submit();
    assert!(!app.shows_thread());

    // `c` again starts another comment and leaves the existing thread folded.
    let top = app.thread_cursor().thread().cloned();
    press(&mut app, "c");
    assert!(top.as_ref().is_some_and(|id| !app.is_expanded(id)));
    assert!(
        matches!(app.popup(), Some(Popup::Compose(c)) if matches!(c.target(), ComposeTarget::New(_)))
    );
    type_in(&mut app, "top again");
    app.compose_submit();
    assert_eq!(app.thread_counts(), (3, 3));

    // The walk is by line, then store order, and moves the cursor.
    app.expand_at_cursor();
    assert_eq!(app.thread_position(), Some((1, 3)));
    app.thread_step_in_file(1);
    assert_eq!(app.thread_position(), Some((2, 3)));
    assert_eq!(app.view().cursor_source_line(), Some(1));
    app.thread_step_in_file(1);
    assert_eq!(app.thread_position(), Some((3, 3)));
    assert_eq!(app.view().cursor_source_line(), bottom);
    app.thread_step_in_file(1);
    assert_eq!(app.thread_position(), Some((1, 3)), "wraps");
    assert_eq!(app.view().cursor_source_line(), Some(1));
    app.thread_step_in_file(-1);
    assert_eq!(app.view().cursor_source_line(), bottom);
    Ok(())
}

#[test]
fn resolving_in_the_list_keeps_the_scroll_and_moves_to_the_next_entry() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-resolve-scroll", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 16);
    // Six threads on the file's six non-blank lines, each four rows
    // tall, so the list scrolls.
    app.view_mut().toggle_source_view();
    for line in [1, 3, 4, 5, 7, 8] {
        app.view_mut().goto_source_line(line);
        app.start_comment();
        type_in(&mut app, &format!("thread {line}\nmore\nmore"));
        app.compose_submit();
    }
    app.open_review();
    let rows = app.review_rows(app.column_width());
    assert_eq!(rows.entries.len(), 6);
    // `gg` lands on the file row (ADR 0076); four steps reach the
    // fourth entry.
    app.review_goto(false);
    for _ in 0..4 {
        app.review_step(1);
    }
    let scroll = app.review_list().scroll();
    assert!(scroll > 0, "the fourth entry is below the fold");
    let fourth = rows.entries[3].id().clone();
    let fifth = rows.entries[4].id().clone();
    assert_eq!(app.thread_cursor().thread(), Some(&fourth));
    // `o` hides the resolved entry; the one below it takes its place and
    // the rows keep still under the eye.
    app.thread_toggle_resolved();
    assert_eq!(app.review_rows(app.column_width()).entries.len(), 5);
    assert_eq!(app.thread_cursor().thread(), Some(&fifth));
    assert_eq!(app.review_list().scroll(), scroll, "no jump");
    Ok(())
}

/// ADR 0071: the cursor bar is the first cell of the cursor's thread's
/// entry header and of every row of the cursor's message in the list,
/// and of the file row over them (ADR 0077), the thread's rows in the
/// nest under the path; in the file the expanded thread's rows begin
/// with a gutter of their own, the bar in its first cell on the header
/// and the cursor's message.
#[test]
fn the_cursor_bar_marks_the_thread_and_its_message_on_both_surfaces() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-cursor-bar", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app, "opening\nsecond line")?;
    let id = app.marks()[0].id().clone();
    testing::external_agent_reply(
        &mut app,
        &id,
        Author::agent("reviewer"),
        "agent answer",
        false,
        None,
    )?;
    app.expand_thread(id.clone());
    app.thread_reply();
    type_in(&mut app, "user follow-up");
    app.compose_submit();
    app.open_review();
    assert_eq!(app.thread_cursor().thread(), Some(&id));
    assert_eq!(app.thread_cursor().message(), 2, "the newest message");
    let rows = testing::screen(&app)?;
    let marked: Vec<&String> = rows
        .iter()
        .filter(|row| row.starts_with('▎') && !row.starts_with("▎Threads"))
        .collect();
    // The file row the cursor is within, not bold on its own (ADR
    // 0077), the entry header, then the newest message's author row
    // and body row, each in the nest.
    assert_eq!(marked.len(), 4, "{rows:?}");
    assert!(marked[0].starts_with("▎▾ README.md"), "{marked:?}");
    assert!(
        marked[1].starts_with("▎  ● ▾") && marked[1].contains("L3"),
        "{marked:?}"
    );
    assert!(marked[2].starts_with("▎    User  "), "{marked:?}");
    assert!(marked[3].starts_with("▎      user follow-up"), "{marked:?}");
    let other = rows
        .iter()
        .find(|row| row.contains("agent answer"))
        .context("the agent's body row")?;
    assert!(other.starts_with("       agent answer"), "{other:?}");
    let author = rows
        .iter()
        .find(|row| row.contains("reviewer"))
        .context("the agent's author row")?;
    assert!(author.starts_with("     reviewer  "), "{author:?}");
    // File scope drops the file row and keeps the nest.
    app.review_toggle_file();
    let rows = testing::screen(&app)?;
    assert!(
        !rows.iter().any(|row| row.contains("▾ README.md")),
        "no file row in file scope: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.starts_with("▎  ● ▾")),
        "the threads keep the nest in file scope: {rows:?}"
    );
    app.review_toggle_file();

    // In the file: the global gutter, then the thread's own.
    app.thread_open_in_file();
    let rows = testing::screen(&app)?;
    let gutter = crate::app::draw::gutter_width(app.view());
    let after = |row: &str| row.chars().skip(gutter).collect::<String>();
    let header = rows
        .iter()
        .find(|row| after(row).starts_with("▎● ▾"))
        .with_context(|| format!("the expanded header: {rows:?}"))?;
    assert!(!header.contains("Resolve"), "{header:?}");
    assert!(
        rows[app.text_bar_row()].contains("resolve r"),
        "{:?}",
        rows[app.text_bar_row()]
    );
    let follow = rows
        .iter()
        .find(|row| row.contains("user follow-up"))
        .context("the cursor's body row")?;
    assert!(
        after(follow).starts_with("▎   user follow-up"),
        "{follow:?}"
    );
    let answer = rows
        .iter()
        .find(|row| row.contains("agent answer"))
        .context("the agent's body row")?;
    assert!(after(answer).starts_with("    agent answer"), "{answer:?}");
    let opening = rows
        .iter()
        .find(|row| row.contains("second line"))
        .context("the comment's second row")?;
    assert!(after(opening).starts_with("    second line"), "{opening:?}");
    Ok(())
}

#[test]
fn the_review_list_renders_bodies_as_markdown() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-list-markdown", testing::README)?;
    let mut app = app(&dir)?;
    app.start_comment();
    type_in(&mut app, "Two **points** and `code`");
    app.compose_submit();
    app.open_review();
    let rows = app.review_rows(60);
    let bodies: Vec<&Line> = rows
        .rows
        .iter()
        .filter_map(|row| match row {
            Row::Body { line, .. } => Some(line),
            _ => None,
        })
        .collect();
    // The markers are gone as in the expanded thread (ADR 0037): the
    // asterisks and backticks are not drawn, and the code span keeps
    // its face.
    assert_eq!(bodies.len(), 1, "{bodies:?}");
    assert_eq!(bodies[0].text(), "Two points and code");
    assert!(
        bodies[0]
            .spans()
            .iter()
            .any(|span| span.text() == "code" && span.style().face == Face::Code),
        "{bodies:?}"
    );
    Ok(())
}

#[test]
fn review_navigation_reuses_shared_message_layouts() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-list-layout-cache", testing::README)?;
    let mut app = app(&dir)?;
    app.highlighter = Arc::new(Highlighter::new("base16-ocean.dark")?);
    let markdown = format!(
        "```markdown\n{}\n```",
        "[reference](https://example.com/path) **strong** `inline-code` ".repeat(10)
    );
    for _ in 0..8 {
        app.view_mut().goto_source_line(3);
        app.start_comment();
        app.compose_insert(&markdown);
        app.compose_submit();
    }

    app.open_review();
    let rendered = app.message_layout_cache.renders();
    assert_eq!(rendered, 8, "each thread is initially rendered once");
    let started = Instant::now();
    for _ in 0..20 {
        black_box(app.review_rows(app.column_width()));
    }
    assert_eq!(
        app.message_layout_cache.renders(),
        rendered,
        "review row rebuilds reuse prepared Markdown"
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "cached review row rebuilds took {:?}",
        started.elapsed()
    );

    let id = app.marks()[0].id().clone();
    app.close_review();
    app.expand_thread(id);
    let after_file = app.message_layout_cache.renders();
    let other_threads = app.marks()[1..]
        .iter()
        .map(|mark| mark.id().clone())
        .collect::<Vec<_>>();
    app.review_list.folded_threads.extend(other_threads);
    let inline_body_width = app
        .view()
        .layout()
        .width()
        .saturating_sub(MESSAGE_INDENT)
        .max(1);
    black_box(app.review_rows(inline_body_width + BODY_INDENT));
    assert_eq!(
        app.message_layout_cache.renders(),
        after_file,
        "File and Reviews share a matching effective body width"
    );
    Ok(())
}

#[test]
fn the_review_list_shows_the_work_and_acts_in_place() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-list", testing::README)?;
    let mut app = app(&dir)?;
    app.start_comment();
    type_in(&mut app, "top");
    app.compose_submit();
    app.view_mut().goto_bottom();
    app.start_comment();
    type_in(&mut app, "bottom");
    app.compose_submit();
    let bottom = app.view().cursor_source_line();
    app.view_mut().goto_top();

    // The list (ADR 0025, ADR 0049) takes the column: both threads,
    // every header carrying the path; Enter opens the file with the
    // thread expanded.
    app.open_review();
    assert_eq!(app.focus(), Focus::Review);
    assert!(!app.shows_thread());
    let rows = app.review_rows(60);
    assert_eq!(rows.entries.len(), 2);
    assert!(matches!(
        rows.rows.first(),
        Some(Row::File { path, count: 2, .. }) if path == Path::new("README.md")
    ));
    assert!(
        rows.rows
            .iter()
            .any(|row| matches!(row, Row::Body { line, .. } if line.text() == "top"))
    );
    app.review_step(1);
    app.thread_open_in_file();
    assert!(!app.review_list().is_open());
    assert_eq!(app.focus(), Focus::View);
    assert!(app.shows_thread());
    assert_eq!(app.view().cursor_source_line(), bottom);

    // `o` resolves an entry, which leaves the list until `x` shows
    // it dimmed; `f` narrows to the file; reopening keeps the entry.
    app.open_review();
    app.thread_toggle_resolved();
    assert_eq!(app.message(), Some("resolved"));
    assert_eq!(app.review_rows(60).entries.len(), 1, "resolved hidden");
    app.review_toggle_resolved();
    let rows = app.review_rows(60);
    assert_eq!(rows.entries.len(), 2);
    assert!(
        rows.rows
            .iter()
            .any(|row| matches!(row, Row::Header { dim: true, .. }))
    );
    app.review_toggle_resolved();
    app.review_toggle_file();
    assert_eq!(app.review_rows(60).entries.len(), 1);
    app.review_toggle_file();
    app.close_review();
    assert_eq!(app.focus(), Focus::View);
    app.open_review();
    app.thread_reply();
    type_in(&mut app, "still here");
    app.compose_submit();
    assert_eq!(app.focus(), Focus::Review);
    assert!(app.review_list().is_open());
    assert!(
        app.review_rows(60)
            .rows
            .iter()
            .any(|row| matches!(row, Row::Body { line, .. } if line.text() == "still here"))
    );
    // A file opened by any route takes the column back.
    app.open(Path::new("README.md"));
    assert!(!app.review_list().is_open());
    assert_eq!(app.focus(), Focus::View);

    Ok(())
}

/// The review lists files in the files pane's order under a row per
/// file, threads by line within one (ADR 0066); `z` folds the cursor's
/// file to its row and `Z` every file, a folded file being one stop.
/// A workspace with three threads for the review list: `guide` in
/// docs/guide.md, then `top` and `bottom` in README.md, the bottom one
/// answered by an agent; the list open in workspace scope.
fn review_by_file(
    name: &str,
) -> anyhow::Result<(TempDir, App, [fathomable_core::annotations::ThreadId; 3])> {
    let dir = testing::workspace(name, testing::README)?;
    fs::create_dir_all(dir.0.join("ws/docs"))?;
    fs::write(dir.0.join("ws/docs/guide.md"), "# Guide\n\nfirst\n")?;
    let mut app = app(&dir)?;
    app.view_mut().goto_bottom();
    app.start_comment();
    type_in(&mut app, "bottom");
    app.compose_submit();
    app.view_mut().goto_top();
    app.start_comment();
    type_in(&mut app, "top");
    app.compose_submit();
    app.open(Path::new("docs/guide.md"));
    app.view_mut().goto_bottom();
    app.start_comment();
    type_in(&mut app, "guide");
    app.compose_submit();
    let guide = app.file_threads()[0].clone();
    app.open(Path::new("README.md"));
    let top = app.file_threads()[0].clone();
    let bottom = app.file_threads()[1].clone();
    testing::external_agent_reply(
        &mut app,
        &bottom,
        Author::agent("reviewer"),
        "answered",
        false,
        None,
    )?;
    app.open_review();
    Ok((dir, app, [guide, top, bottom]))
}

/// The list's entries in order.
fn review_order(app: &App) -> Vec<fathomable_core::annotations::ThreadId> {
    app.review_rows(60)
        .entries
        .iter()
        .map(|entry| entry.id().clone())
        .collect()
}

/// The list's file rows, `▸ ` before a folded one.
fn review_files(app: &App) -> Vec<String> {
    app.review_rows(60)
        .rows
        .iter()
        .filter_map(|row| match row {
            Row::File { path, folded, .. } => Some(format!(
                "{}{}",
                if *folded { "▸ " } else { "" },
                path.display()
            )),
            _ => None,
        })
        .collect()
}

/// Whether the cursor rests on `path`'s file row.
fn review_file_selected(app: &App, path: &str) -> bool {
    app.review_rows(60)
        .rows
        .iter()
        .any(|row| matches!(row, Row::File { path: p, selected: true, .. } if p == Path::new(path)))
}

#[test]
fn the_review_groups_by_file_and_folds_files() -> anyhow::Result<()> {
    let (_dir, mut app, [guide, top, bottom]) = review_by_file("threads-by-file")?;
    assert_eq!(
        review_order(&app),
        [guide.clone(), top.clone(), bottom.clone()],
        "the directory first, then lines; the reply does not reorder"
    );
    assert_eq!(review_files(&app), ["docs/guide.md", "README.md"]);

    // `k` from README's first thread stops on README's file row (ADR
    // 0076), where `z` folds the file; the cursor stays on the row over
    // the file's first thread, `k` reaches the guide's thread and then
    // its row, `j` comes back to README's row once, and no further.
    app.set_thread_cursor(top.clone());
    app.review_step(-1);
    assert!(
        review_file_selected(&app, "README.md"),
        "the file row is a stop"
    );
    app.review_fold();
    assert_eq!(review_files(&app), ["docs/guide.md", "▸ README.md"]);
    assert!(review_file_selected(&app, "README.md"));
    assert!(
        !app.review_rows(60)
            .rows
            .iter()
            .any(|row| matches!(row, Row::Header { entry: 1, .. })),
        "a folded file's threads have no rows"
    );
    assert_eq!(app.thread_cursor().thread(), Some(&top));
    app.review_step(-1);
    assert_eq!(app.thread_cursor().thread(), Some(&guide));
    assert!(!review_file_selected(&app, "docs/guide.md"));
    app.review_step(-1);
    assert!(review_file_selected(&app, "docs/guide.md"));
    app.review_step(-1);
    assert!(
        review_file_selected(&app, "docs/guide.md"),
        "the first stop"
    );
    app.review_step(2);
    assert!(review_file_selected(&app, "README.md"));
    assert_eq!(app.thread_cursor().thread(), Some(&top));
    app.review_step(1);
    assert!(
        review_file_selected(&app, "README.md"),
        "one stop, no further"
    );
    app.review_fold();
    assert_eq!(
        review_files(&app),
        ["docs/guide.md", "README.md"],
        "`z` unfolds"
    );
    assert!(
        review_file_selected(&app, "README.md"),
        "the cursor stays on the row"
    );
    // `f` lists one file with no file row.
    app.review_toggle_file();
    assert!(review_files(&app).is_empty());
    assert_eq!(review_order(&app), [top.clone(), bottom.clone()]);
    Ok(())
}

/// ADR 0076: `z` on a thread folds it to one packed row and expands it
/// again; `Z` folds every thread while any is expanded and expands
/// them all when every one is folded, leaving the files alone; and `z`
/// keeps its thread meaning in file scope.
#[test]
fn the_review_folds_threads_and_shift_z_every_thread() -> anyhow::Result<()> {
    let (_dir, mut app, [guide, top, bottom]) = review_by_file("threads-fold-list")?;
    app.set_thread_cursor(top.clone());
    app.review_fold();
    assert!(app.review_list().is_thread_folded(&top));
    let rows = app.review_rows(60);
    let stub = rows
        .rows
        .iter()
        .position(|row| {
            matches!(
                row,
                Row::Stub {
                    entry: 1,
                    selected: true,
                    ..
                }
            )
        })
        .context("the folded row")?;
    assert!(
        matches!(rows.rows[stub + 1], Row::Header { entry: 2, .. }),
        "packed, no blank row: {:?}",
        rows.rows[stub + 1]
    );
    app.review_fold();
    assert!(!app.review_list().is_thread_folded(&top));

    let all_folded = |app: &App| {
        [&guide, &top, &bottom]
            .iter()
            .all(|id| app.review_list().is_thread_folded(id))
    };
    app.review_fold_all();
    assert!(all_folded(&app));
    assert_eq!(review_files(&app), ["docs/guide.md", "README.md"]);
    app.review_toggle_thread(&top);
    app.review_fold_all();
    assert!(all_folded(&app), "any expanded: fold them all");
    app.review_fold_all();
    assert!(
        [&guide, &top, &bottom]
            .iter()
            .all(|id| !app.review_list().is_thread_folded(id)),
        "all folded: expand them all"
    );
    app.review_toggle_file();
    app.set_thread_cursor(top.clone());
    app.review_fold();
    assert!(
        app.review_list().is_thread_folded(&top),
        "`z` in file scope"
    );
    Ok(())
}

#[test]
fn externally_written_reply_is_observed() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-external-reply", testing::README)?;
    fs::write(dir.0.join("ws/other.md"), "# Other\n\nline\n")?;
    let mut app = app(&dir)?;
    app.view_mut().move_down(2);
    app.view_mut().select_lines();
    app.start_comment();
    type_in(&mut app, "please check");
    app.compose_submit();
    let id = app.marks()[0].id().clone();

    let author = Author::Agent {
        name: "reviewer".to_owned(),
        client: Some("claude-code".to_owned()),
        id: None,
    };
    let resolution =
        testing::external_agent_reply(&mut app, &id, author.clone(), "fixed", true, None)?;
    assert_eq!(
        resolution,
        fathomable_core::annotations::ResolutionOutcome::ResolutionProposed
    );
    assert_eq!(
        app.thread(&id).map(|thread| thread.replies().len()),
        Some(1)
    );
    assert_eq!(
        app.toasts().last().map(crate::app::Toast::text),
        Some("reviewer replied and proposed resolution on README.md:3")
    );
    let thread = app
        .thread(&id)
        .ok_or_else(|| anyhow::anyhow!("thread lost"))?;
    // The agent proposed; the thread stays unresolved.
    assert_eq!(thread.status(), Status::Open);
    assert!(thread.proposes_resolution());
    assert_eq!(
        thread.lifecycle(),
        fathomable_core::annotations::Lifecycle::ResolutionProposed
    );
    assert_eq!(thread.replies()[0].author(), &author);
    assert!(thread.replies()[0].proposes_resolution());
    assert_eq!(
        thread.replies()[0].author().to_string(),
        "reviewer (claude-code)"
    );
    app.open(Path::new("README.md"));
    assert_eq!(app.thread_counts(), (1, 1));
    assert_eq!(app.proposed_count(), 1);

    let unknown = serde_json::from_str(r#""9-9-9""#)?;
    testing::external_agent_reply(&mut app, &unknown, author, "?", false, None)
        .err()
        .context("unknown thread should fail")?;
    Ok(())
}

#[test]
fn external_reply_consumes_auto_resolve_and_uses_current_head() -> anyhow::Result<()> {
    use fathomable_core::annotations::{AutoResolve, ResolutionOutcome};
    use fathomable_core::workspace::Workspace;

    let dir = testing::workspace("threads-external-resolve", testing::README)?;
    let root = testing::root(&dir);
    fathomable_testing::git::init(&root)?;
    fathomable_testing::git::commit_and_stage(&root, &[("README.md", testing::README)])?;
    let head = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("missing HEAD"))?;
    let mut app = app(&dir)?;
    annotate(&mut app, "finish this")?;
    let id = app.marks()[0].id().clone();
    app.store_mut()
        .ok_or_else(|| anyhow::anyhow!("store"))?
        .set_auto_resolve(&id, AutoResolve::Enabled, 1)?;

    let resolution = testing::external_agent_reply(
        &mut app,
        &id,
        Author::agent("reviewer"),
        "fixed",
        true,
        None,
    )?;
    let thread = app.thread(&id).context("thread")?;
    assert_eq!(resolution, ResolutionOutcome::Resolved);
    assert_eq!(thread.status(), Status::Resolved);
    assert_eq!(
        thread.auto_resolve(),
        fathomable_core::annotations::AutoResolve::Disabled
    );
    assert_eq!(thread.commit(), Some(head.as_str()));
    assert_eq!(
        app.toasts().last().map(crate::app::Toast::text),
        Some("reviewer replied and resolved README.md:3")
    );

    Ok(())
}

#[test]
fn external_plain_reply_consumes_one_shot_permission_without_resolving() -> anyhow::Result<()> {
    use fathomable_core::annotations::AutoResolve;

    let dir = testing::workspace("threads-external-consume", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "finish this")?;
    let id = app.marks()[0].id().clone();
    app.store_mut()
        .ok_or_else(|| anyhow::anyhow!("store"))?
        .set_auto_resolve(&id, AutoResolve::Enabled, 1)?;

    testing::external_agent_reply(
        &mut app,
        &id,
        Author::agent("reviewer"),
        "work in progress",
        false,
        None,
    )?;
    let thread = app.thread(&id).context("thread")?;
    assert_eq!(thread.status(), Status::Open);
    assert_eq!(thread.auto_resolve(), AutoResolve::Disabled);
    assert!(!thread.proposes_resolution());
    Ok(())
}

/// An externally stored opening appears without disturbing local UI state.
#[test]
fn externally_written_start_is_observed() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-external-start", testing::README)?;
    let mut app = app(&dir)?;
    let before = app.view().cursor_source_line();
    let author = Author::agent("reviewer");
    let id = Store::open(testing::store_path(&dir))?.annotate(
        Draft::new(
            author.clone(),
            Path::new("README.md"),
            LineRange::new(2, 3),
            "look here",
        ),
        testing::README,
        1,
    )?;
    app.reload_store();
    let started = app.thread(&id).context("external thread")?;
    assert_eq!(started.author(), &author);
    assert_eq!(started.comment(), "look here");
    assert_eq!(started.range(), Some(LineRange::new(2, 3)));
    assert_eq!(
        started.lifecycle(),
        fathomable_core::annotations::Lifecycle::Active
    );
    assert_eq!(
        app.toasts().last().map(crate::app::Toast::text),
        Some("reviewer started a thread on README.md:2")
    );
    assert_eq!(app.view().cursor_source_line(), before);
    assert_eq!(
        app.message_for(&id, MessageTarget::Comment),
        Some(("look here", false))
    );
    // The review list names the agent on the comment's row as the
    // toast does, and the user's reply by the configured name (ADR 0058).
    app.open_review();
    app.thread_reply();
    type_in(&mut app, "noted");
    app.compose_submit();
    app.open_review();
    let rows = app.review_rows(100);
    let authors: Vec<&str> = rows
        .rows
        .iter()
        .filter_map(|row| match row {
            Row::Message { author, .. } => Some(author.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(authors, ["reviewer", "User"], "{:?}", rows.rows);
    app.close_review();

    Ok(())
}

#[test]
fn external_reload_preserves_local_reading_and_draft_state() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-external-preserves-ui", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "question")?;
    let id = app.file_threads()[0].clone();
    app.expand_thread(id.clone());
    app.view_mut().goto_source_line(4);
    app.view_mut().select_lines();
    app.thread_reply();
    type_in(&mut app, "unfinished local draft");
    let focus = app.focus();
    let cursor = app.view().cursor();
    let selection = app.view().selection();

    let mut writer = Store::open(testing::store_path(&dir))?;
    writer.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("reviewer"), 10, "external answer"),
        |_| Ok(testing::README.to_owned()),
    )?;
    app.reload_store();

    assert_eq!(app.compose_draft(), Some("unfinished local draft"));
    assert_eq!(app.focus(), focus);
    assert_eq!(app.view().cursor(), cursor);
    assert_eq!(app.view().selection(), selection);
    assert!(app.is_expanded(&id));
    assert_eq!(
        app.thread(&id).map(|thread| thread.replies().len()),
        Some(1)
    );
    Ok(())
}

#[test]
fn checkout_reads_reject_symlink_escapes_and_nonrelative_paths() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;

    let dir = testing::workspace("start-symlink-escape", testing::README)?;
    let mut workspace = fathomable_core::workspace::Workspace::discover(testing::root(&dir))?;
    let outside = dir.0.join("outside");
    fs::create_dir(&outside)?;
    fs::write(outside.join("a.md"), "synthetic outside-only evidence\n")?;
    symlink("../outside/a.md", testing::root(&dir).join("escape.md"))?;
    symlink("../outside", testing::root(&dir).join("linked-directory"))?;
    for path in [
        PathBuf::from("escape.md"),
        PathBuf::from("linked-directory/a.md"),
        PathBuf::from("../outside/a.md"),
        outside.join("a.md"),
    ] {
        for range in [Some(LineRange::new(1, 1)), None] {
            let response = super::agent_start_draft(
                &mut workspace,
                Author::agent("reviewer"),
                &path,
                range,
                "must not capture outside text".to_owned(),
            );
            assert!(response.is_err(), "{response:?}");
            assert!(!format!("{response:?}").contains("outside-only evidence"));
            assert!(Store::open(testing::store_path(&dir))?.threads().is_empty());
        }
    }
    Ok(())
}

#[test]
fn external_replies_reject_symlink_replacement_without_mutating_thread() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;

    let dir = testing::workspace("reply-symlink-escape", testing::README)?;
    let mut app = app(&dir)?;
    annotate(&mut app, "original review")?;
    let id = app.marks()[0].id().clone();
    let before = fs::read(testing::store_path(&dir))?;
    fs::write(
        dir.0.join("outside.txt"),
        "synthetic outside-only evidence\nsecond private line\n",
    )?;
    fs::remove_file(testing::root(&dir).join("README.md"))?;
    symlink("../outside.txt", testing::root(&dir).join("README.md"))?;

    let response = testing::external_agent_reply(
        &mut app,
        &id,
        Author::agent("reviewer"),
        "must not relocate to outside text",
        false,
        Some(LineRange::new(2, 2)),
    );
    assert!(response.is_err(), "{response:?}");
    assert!(!format!("{response:?}").contains("outside-only evidence"));
    assert_eq!(fs::read(testing::store_path(&dir))?, before);
    Ok(())
}

#[test]
fn external_idempotent_start_replay_does_not_toast_twice() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-external-idempotent-start", testing::README)?;
    let mut app = app(&dir)?;
    let author = Author::Agent {
        name: "reviewer".to_owned(),
        client: Some("copilot-cli".to_owned()),
        id: Some("copilot:viewer-start".to_owned()),
    };
    let draft = Draft::new(
        author,
        Path::new("README.md"),
        LineRange::new(3, 2),
        "same request",
    );
    let mut writer = Store::open(testing::store_path(&dir))?;
    let first = writer.annotate_idempotent_for_caller(
        draft.clone(),
        1,
        "test:viewer",
        "start-once",
        |_| Ok(testing::README.to_owned()),
    )?;
    let id = first.into_value();
    app.reload_store();
    let toast_count = app.toasts().len();
    std::fs::remove_file(dir.0.join("ws/README.md"))?;

    let replay = Store::open(testing::store_path(&dir))?.annotate_idempotent_for_caller(
        draft,
        2,
        "test:viewer",
        "start-once",
        |_| {
            Err(fathomable_core::annotations::StoreError::message(
                "replay must not read",
            ))
        },
    )?;
    assert!(replay.replayed());
    assert_eq!(replay.value(), &id);
    app.reload_store();
    assert_eq!(
        app.thread(&id)
            .map(fathomable_core::annotations::Thread::comment),
        Some("same request")
    );
    assert_eq!(app.toasts().len(), toast_count);
    Ok(())
}

#[test]
fn external_idempotent_reply_replay_does_not_toast_twice() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-external-idempotent-reply", testing::README)?;
    let mut app = app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    app.compose_insert("question");
    app.compose_submit();
    let id = app.file_threads()[0].clone();
    let author = Author::Agent {
        name: "reviewer".to_owned(),
        client: Some("copilot-cli".to_owned()),
        id: Some("copilot:viewer-reply".to_owned()),
    };
    let command = AgentReplyCommand::new(author, 1, "answer")
        .relocate(LineRange::new(3, 3))
        .idempotent("test:viewer", "reply-once");
    let mut writer = Store::open(testing::store_path(&dir))?;
    let first = writer.agent_reply(&id, command.clone(), |_| Ok(testing::README.to_owned()))?;
    assert!(!first.replayed());
    app.reload_store();
    let toast_count = app.toasts().len();
    std::fs::remove_file(dir.0.join("ws/README.md"))?;

    let replay = Store::open(testing::store_path(&dir))?.agent_reply(&id, command, |_| {
        Err(fathomable_core::annotations::StoreError::message(
            "replay must not read",
        ))
    })?;
    assert!(replay.replayed());
    app.reload_store();
    assert_eq!(
        app.thread(&id).map(|thread| thread.replies().len()),
        Some(1)
    );
    assert_eq!(app.toasts().len(), toast_count);
    Ok(())
}

#[test]
fn startup_seeds_historical_agent_activity_without_toasting() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-startup", testing::README)?;
    let mut store = Store::open(testing::store_path(&dir))?;
    let id = store.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("README.md"),
            LineRange::new(2, 3),
            "finding",
        ),
        testing::README,
        1,
    )?;
    store.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("reviewer"), 2, "more"),
        |_| Ok(testing::README.to_owned()),
    )?;

    let app = app(&dir)?;
    assert!(app.toasts().is_empty());
    Ok(())
}

#[test]
fn reloaded_agent_opening_toasts_once_with_author_and_place() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-reload-opening", testing::README)?;
    let mut app = app(&dir)?;
    let mut writer = Store::open(testing::store_path(&dir))?;
    writer.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("README.md"),
            LineRange::new(3, 4),
            "finding",
        ),
        testing::README,
        1,
    )?;

    app.reload_store();
    assert_eq!(
        app.toasts().last().map(crate::app::Toast::text),
        Some("reviewer started a thread on README.md:3")
    );
    let count = app.toasts().len();
    app.reload_store();
    assert_eq!(app.toasts().len(), count, "unchanged refresh is silent");
    Ok(())
}

#[test]
fn consecutive_agent_replies_on_one_thread_each_toast() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-consecutive", testing::README)?;
    let mut store = Store::open(testing::store_path(&dir))?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(3, 3),
            "question",
        ),
        testing::README,
        1,
    )?;
    let mut app = app(&dir)?;
    let mut writer = Store::open(testing::store_path(&dir))?;

    writer.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("reviewer"), 2, "first"),
        |_| Ok(testing::README.to_owned()),
    )?;
    app.reload_store();
    assert_eq!(
        app.toasts().last().map(crate::app::Toast::text),
        Some("reviewer replied on README.md:3")
    );

    writer.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("reviewer"), 3, "second"),
        |_| Ok(testing::README.to_owned()),
    )?;
    app.reload_store();
    assert_eq!(app.toasts().len(), 2);
    assert_eq!(
        app.toasts().last().map(crate::app::Toast::text),
        Some("reviewer replied on README.md:3")
    );
    Ok(())
}

#[test]
fn multiple_agent_activities_aggregate_into_one_toast() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-aggregate", testing::README)?;
    let mut app = app(&dir)?;
    let mut writer = Store::open(testing::store_path(&dir))?;
    for (line, name) in [(2, "alpha"), (4, "beta")] {
        writer.annotate(
            Draft::new(
                Author::agent(name),
                Path::new("README.md"),
                LineRange::new(line, line),
                "finding",
            ),
            testing::README,
            line as u64,
        )?;
    }

    app.reload_store();
    assert_eq!(app.toasts().len(), 1);
    assert_eq!(app.toasts()[0].text(), "2 agent updates");
    Ok(())
}

#[test]
fn reloaded_user_activity_does_not_toast() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-user", testing::README)?;
    let mut app = app(&dir)?;
    Store::open(testing::store_path(&dir))?.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(3, 3),
            "note",
        ),
        testing::README,
        1,
    )?;

    app.reload_store();
    assert!(app.toasts().is_empty());
    Ok(())
}

#[test]
fn agent_activity_imported_during_user_write_is_not_missed() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-user-write-import", testing::README)?;
    let mut store = Store::open(testing::store_path(&dir))?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(3, 3),
            "question",
        ),
        testing::README,
        1,
    )?;
    let mut app = app(&dir)?;
    Store::open(testing::store_path(&dir))?.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("reviewer"), 2, "answer"),
        |_| Ok(testing::README.to_owned()),
    )?;

    app.set_thread_cursor(id);
    app.thread_reply();
    type_in(&mut app, "thanks");
    app.compose_submit();

    assert_eq!(
        app.toasts().last().map(crate::app::Toast::text),
        Some("reviewer replied on README.md:3")
    );
    Ok(())
}

#[test]
fn deleted_agent_activity_keeps_its_durable_place() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-deleted", testing::README)?;
    let mut store = Store::open(testing::store_path(&dir))?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(4, 5),
            "question",
        ),
        testing::README,
        1,
    )?;
    let mut app = app(&dir)?;
    let mut writer = Store::open(testing::store_path(&dir))?;
    writer.agent_reply(
        &id,
        AgentReplyCommand::new(Author::agent("reviewer"), 2, "answer"),
        |_| Ok(testing::README.to_owned()),
    )?;
    writer.delete(&id, 3)?;

    app.reload_store();
    assert_eq!(
        app.toasts().last().map(crate::app::Toast::text),
        Some("reviewer replied on README.md:4")
    );
    Ok(())
}

#[test]
fn deleted_agent_opening_still_requests_a_redraw_once() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-deleted-opening", testing::README)?;
    let mut app = app(&dir)?;
    let mut writer = Store::open(testing::store_path(&dir))?;
    let id = writer.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("README.md"),
            LineRange::new(4, 5),
            "transient finding",
        ),
        testing::README,
        1,
    )?;
    writer.delete(&id, 2)?;

    let reload = app.reload_store();
    assert!(reload.healthy);
    assert!(reload.changed, "the new activity toast needs a redraw");
    assert!(app.thread(&id).is_none());
    assert_eq!(
        app.toasts().last().map(crate::app::Toast::text),
        Some("reviewer started a thread on README.md:4")
    );

    let toast_count = app.toasts().len();
    let duplicate = app.reload_store();
    assert!(duplicate.healthy);
    assert!(!duplicate.changed, "the same activity is not redrawn twice");
    assert_eq!(app.toasts().len(), toast_count);
    Ok(())
}

#[test]
fn activity_store_replacement_and_cursor_regression_do_not_replay_history() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-reset", testing::README)?;
    let path = testing::store_path(&dir);
    let mut initial = Store::open(&path)?;
    initial.annotate(
        Draft::new(
            Author::agent("historical"),
            Path::new("README.md"),
            LineRange::new(2, 2),
            "old",
        ),
        testing::README,
        1,
    )?;
    let mut app = app(&dir)?;
    assert!(app.toasts().is_empty(), "startup seeds at the end");
    Store::open(&path)?.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("README.md"),
            LineRange::new(3, 3),
            "new",
        ),
        testing::README,
        2,
    )?;
    app.reload_store();
    assert_eq!(app.toasts().len(), 1);

    let replacement = dir.0.join("state/replacement.jsonl");
    Store::open(&replacement)?.annotate(
        Draft::new(
            Author::agent("replacement-history"),
            Path::new("README.md"),
            LineRange::new(4, 4),
            "replacement",
        ),
        testing::README,
        3,
    )?;
    fs::rename(&replacement, &path)?;
    app.reload_store();
    assert_eq!(
        app.toasts().len(),
        1,
        "cursor regression reseeds instead of replaying"
    );
    Ok(())
}

#[test]
fn missing_or_invalid_backing_keeps_last_good_board_and_cursor() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-last-good", testing::README)?;
    let path = testing::store_path(&dir);
    let mut writer = Store::open(&path)?;
    let id = writer.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("README.md"),
            LineRange::new(3, 3),
            "keep this",
        ),
        testing::README,
        1,
    )?;
    let mut app = app(&dir)?;
    let cursor = app.activity_cursor;

    fs::remove_file(&path)?;
    let missing = app.reload_store();
    assert!(!missing.healthy);
    assert!(missing.changed);
    assert!(
        !app.reload_store().changed,
        "identical failure is suppressed"
    );
    assert!(app.thread(&id).is_some());
    assert_eq!(app.activity_cursor, cursor);
    assert!(!path.exists(), "reload must not recreate a missing store");
    app.toggle_auto_resolve(&id);
    assert!(
        !path.exists(),
        "local writes must not recreate a previously observed store"
    );

    fs::write(&path, "{not json}\n")?;
    let corrupt = app.reload_store();
    assert!(!corrupt.healthy);
    assert!(app.thread(&id).is_some());
    assert_eq!(app.activity_cursor, cursor);

    fs::write(&path, "{\"v\":4}\n")?;
    let mismatch = app.reload_store();
    assert!(!mismatch.healthy);
    assert!(app.thread(&id).is_some());
    assert_eq!(app.activity_cursor, cursor);
    Ok(())
}

#[test]
fn first_write_removal_cannot_revert_to_bootstrap() -> anyhow::Result<()> {
    enum Observe {
        Reload,
        Reconcile,
        Write,
    }

    for observe in [Observe::Reload, Observe::Reconcile, Observe::Write] {
        let dir = testing::workspace("activity-first-write-removal", testing::README)?;
        let path = testing::store_path(&dir);
        let mut app = app(&dir)?;
        assert!(!path.exists());
        let id = app.store_mut().context("initial store")?.annotate(
            Draft::new(
                Author::User,
                Path::new("README.md"),
                LineRange::new(3, 3),
                "keep first write",
            ),
            testing::README,
            1,
        )?;
        fs::remove_file(&path)?;

        match observe {
            Observe::Reload => assert!(!app.reload_store().healthy),
            Observe::Reconcile => assert!(!app.reconcile_agent_activity()),
            Observe::Write => assert!(app.store_mut().is_none()),
        }
        let cursor = app.activity_cursor;
        assert!(!app.reload_store().healthy);
        assert!(app.thread(&id).is_some());
        assert_eq!(app.activity_cursor, cursor);
        assert!(app.store_mut().is_none());
        app.reconcile_agent_activity();
        assert!(app.store_mut().is_none());
        assert!(
            !path.exists(),
            "observing a first write must not permit recreating a removed log"
        );
    }
    Ok(())
}

#[test]
fn removal_between_store_open_and_app_creation_preserves_history() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-startup-removal", testing::README)?;
    let path = testing::store_path(&dir);
    let mut loaded = Store::open(&path)?;
    let id = loaded.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(3, 3),
            "keep this",
        ),
        testing::README,
        1,
    )?;
    let cursor = loaded.activity_cursor();
    fs::remove_file(&path)?;

    let mut app = testing::AppBuilder::at(testing::root(&dir))
        .options(move |options| crate::app::Options {
            store: Some(loaded),
            ..options
        })
        .build()?;
    let reload = app.reload_store();
    assert!(!reload.healthy);
    assert!(app.thread(&id).is_some());
    assert_eq!(app.activity_cursor, cursor);

    app.toggle_auto_resolve(&id);
    assert!(
        !path.exists(),
        "a local mutation must not recreate the removed backing"
    );
    Ok(())
}

#[test]
fn removal_of_an_initially_empty_backing_is_not_bootstrap() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-empty-startup-removal", testing::README)?;
    let path = testing::store_path(&dir);
    fathomable_core::private_state::write(&path, "")?;
    let loaded = Store::open(&path)?;
    assert!(loaded.backing_file_observed());
    fs::remove_file(&path)?;

    let mut app = testing::AppBuilder::at(testing::root(&dir))
        .options(move |options| crate::app::Options {
            store: Some(loaded),
            ..options
        })
        .build()?;
    assert!(!app.reload_store().healthy);
    assert!(!path.exists());
    Ok(())
}

#[test]
fn recovery_keeps_rejecting_unsafe_workspace_state_until_fixed() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    use fathomable_core::XdgDirs;
    use fathomable_core::session::{Id, Record};

    let dir = testing::workspace("activity-private-recovery", testing::README)?;
    let root = testing::root(&dir);
    let key = root.clone();
    let state_home = dir.0.join("xdg-state");
    let dirs =
        XdgDirs::resolve(|name| (name == "XDG_STATE_HOME").then(|| state_home.clone().into()));
    let mut writer = Store::open_workspace(&dirs, &key)?;
    let id = writer.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(2, 2),
            "private finding",
        ),
        testing::README,
        1,
    )?;
    let state_dir = dirs.state_dir();
    fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o755))?;
    let startup_error = Store::open_workspace(&dirs, &key)
        .err()
        .context("unsafe application directory should be rejected")?;
    let record = Record::new(Id::mint(), key, root.clone());

    let mut app = testing::AppBuilder::at(root)
        .options(move |options| crate::app::Options {
            record,
            dirs,
            store: None,
            thread_store_error: Some(startup_error),
            ..options
        })
        .build()?;
    let first = app.reload_store();
    assert!(!first.healthy);
    assert!(app.thread(&id).is_none());
    assert_eq!(
        fs::symlink_metadata(&state_dir)?.permissions().mode() & 0o7777,
        0o755,
        "recovery must not repair unsafe permissions"
    );
    assert!(!app.reload_store().healthy);

    fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700))?;
    let recovered = app.reload_store();
    assert!(recovered.healthy);
    assert!(recovered.changed);
    assert!(app.thread(&id).is_some());
    Ok(())
}

#[test]
fn recreated_backing_recovers_and_observes_external_activity() -> anyhow::Result<()> {
    let dir = testing::workspace("activity-recreate", testing::README)?;
    let path = testing::store_path(&dir);
    let mut original = Store::open(&path)?;
    original.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(2, 2),
            "question",
        ),
        testing::README,
        1,
    )?;
    let mut app = app(&dir)?;
    fs::remove_file(&path)?;
    assert!(!app.reload_store().healthy);

    let mut replacement = Store::open(&path)?;
    let id = replacement.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("README.md"),
            LineRange::new(4, 4),
            "new finding",
        ),
        testing::README,
        2,
    )?;
    let reload = app.reload_store();
    assert!(reload.healthy);
    assert!(reload.changed);
    assert!(app.thread(&id).is_some());
    assert_eq!(app.toasts().len(), 0, "cursor regression reseeds silently");
    Ok(())
}

fn draft(app: &App) -> anyhow::Result<(String, Cursor)> {
    let Some(Popup::Compose(compose)) = app.popup() else {
        anyhow::bail!("no draft is open");
    };
    Ok((
        compose.buffer().text().to_owned(),
        compose.buffer().cursor(),
    ))
}

#[test]
fn the_comment_box_edits_around_a_cursor_and_takes_pastes() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-editor", testing::README)?;
    let mut app = app(&dir)?;
    app.view_mut().select_lines();
    app.start_comment();
    type_in(&mut app, "second\nfourth");
    app.compose_edit(Edit::Move(Motion::Up));
    app.compose_edit(Edit::Move(Motion::LineStart));
    app.compose_insert("first ");
    app.paste("\r\nthird\r\n");
    assert_eq!(
        draft(&app)?,
        (
            "first \nthird\nsecond\nfourth".to_owned(),
            Cursor { line: 2, column: 0 }
        )
    );
    app.compose_edit(Edit::DeleteWordBack);
    assert_eq!(draft(&app)?.0, "first \nsecond\nfourth");
    // The editor hatch round-trips the whole draft, cursor at the end.
    assert_eq!(app.compose_draft(), Some("first \nsecond\nfourth"));
    app.set_compose_text("from the editor\nline two");
    assert_eq!(
        draft(&app)?,
        (
            "from the editor\nline two".to_owned(),
            Cursor { line: 1, column: 8 }
        )
    );
    // The draft is a block under L1 (ADR 0054): a header, the author
    // row, and one row per line; with a 100-column pane nothing wraps,
    // and the cursor sits after `line two`.
    let stubs = app.stubs();
    assert_eq!(stubs.len(), 1);
    assert!(matches!(stubs[0].subject(), Subject::Draft(range) if *range == LineRange::new(1, 1)));
    assert_eq!(app.draft_rows(), 3);
    let author = app.draft_author_row().context("the author row")?;
    assert_eq!(
        app.draft_cursor_cell(),
        Some((author + 2, MESSAGE_INDENT + 8))
    );
    app.compose_submit();
    assert!(app.popup().is_none());
    assert!(app.draft_author_row().is_none(), "the draft block is gone");
    assert!(matches!(app.stubs()[0].subject(), Subject::Thread(_)));
    Ok(())
}

#[test]
fn esc_asks_twice_before_discarding_a_draft() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-discard", testing::README)?;
    let mut app = app(&dir)?;
    app.view_mut().select_lines();
    app.start_comment();
    app.compose_cancel();
    assert!(app.popup().is_none(), "an empty box closes at once");
    app.start_comment();
    type_in(&mut app, "keep me");
    app.compose_cancel();
    assert_eq!(app.message(), Some("Esc again to discard the comment"));
    // Typing keeps the draft and drops the prompt.
    type_in(&mut app, "!");
    let Some(Popup::Compose(compose)) = app.popup() else {
        anyhow::bail!("draft was lost");
    };
    assert!(!compose.confirming_discard());
    assert_eq!(compose.buffer().text(), "keep me!");
    app.compose_cancel();
    app.compose_cancel();
    assert!(app.popup().is_none());
    assert_eq!(app.thread_counts(), (0, 0));
    Ok(())
}

#[test]
fn ctrl_c_clears_the_draft_and_closes_an_empty_box() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-clear", testing::README)?;
    let mut app = app(&dir)?;
    app.view_mut().select_lines();
    app.start_comment();
    app.compose_clear();
    assert!(app.popup().is_none(), "an empty box closes at once");
    app.start_comment();
    type_in(&mut app, "wipe me");
    app.compose_clear();
    assert_eq!(draft(&app)?.0, "", "the draft stays open, emptied");
    app.compose_clear();
    assert!(app.popup().is_none());
    assert_eq!(app.thread_counts(), (0, 0));
    Ok(())
}

#[test]
fn a_reply_from_the_review_list_shows_its_row_above_the_key_bar() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-reply-reveal", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 16);
    app.start_comment();
    let long = (1..=30)
        .map(|n| format!("row {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    type_in(&mut app, &long);
    app.compose_submit();
    // Replying from the list opens the file with the thread expanded
    // and the draft at its end, past the bottom of the screen; the
    // draft's row is scrolled on, above the key bar that covers the
    // bottom text row (ADR 0067).
    app.open_review();
    app.thread_reply();
    assert!(matches!(app.popup(), Some(Popup::Compose(_))));
    assert!(app.text_bar_shown());
    let (row, _) = app.draft_cursor_cell().context("the draft's cursor")?;
    let scroll = app.view().scroll();
    assert!(row >= scroll, "{row} < {scroll}");
    assert!(
        row + 1 < scroll + app.text_rows(),
        "row {row} under the bar: scroll {scroll}, {} rows",
        app.text_rows()
    );
    // Typing keeps it there.
    type_in(&mut app, "seen");
    let (row, _) = app.draft_cursor_cell().context("the draft's cursor")?;
    assert!(row + 1 < app.view().scroll() + app.text_rows());
    Ok(())
}

#[test]
fn a_long_draft_wraps_in_its_block_and_the_view_reveals_its_cursor() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-wrap", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 12);
    app.view_mut().select_lines();
    app.start_comment();
    let width = app.draft_width();
    type_in(&mut app, &"x".repeat(width * 10));
    // Ten wrapped rows under the block's header and author row.
    assert_eq!(app.draft_rows(), 11);
    let author = app.draft_author_row().context("the author row")?;
    assert_eq!(app.draft_row_of(author), Some(DraftRow::Author));
    assert_eq!(app.draft_row_of(author + 3), Some(DraftRow::Text(2)));
    assert_eq!(app.draft_row_of(author + 10), Some(DraftRow::Text(9)));
    assert_eq!(app.draft_row_of(author + 11), None);
    // The draft's cursor is at the end of the last row, and the view
    // scrolled to show it while the text cursor stayed on L1 (ADR 0054).
    let (row, col) = app.draft_cursor_cell().context("the cursor")?;
    assert_eq!((row, col), (author + 10, MESSAGE_INDENT + width));
    let scroll = app.view().scroll();
    assert!(scroll > 0, "the view scrolled");
    // Above the key bar on the bottom text row (ADR 0067).
    assert!(
        row >= scroll && row + 1 < scroll + app.text_rows(),
        "revealed"
    );
    assert_eq!(app.view().cursor_source_line(), Some(1));
    // Up on the only line goes to its start, and the view follows the
    // draft's cursor back up; a click lands on the wrapped cell under
    // the pointer.
    app.compose_edit(Edit::Move(Motion::Up));
    assert_eq!(
        app.draft_cursor_cell().map(|(row, _)| row),
        Some(author + 1)
    );
    assert!(app.view().scroll() <= author + 1, "revealed again");
    app.draft_place_cursor(2, MESSAGE_INDENT + 5);
    assert_eq!(
        draft(&app)?.1,
        Cursor {
            line: 0,
            column: width * 2 + 5
        }
    );
    assert_eq!(
        app.draft_cursor_cell(),
        Some((author + 3, MESSAGE_INDENT + 5))
    );
    Ok(())
}

/// Every pane, popup and overlay draws at any terminal size a terminal
/// emulator can report, down to a single cell.
#[test]
fn every_overlay_draws_at_any_terminal_size() -> anyhow::Result<()> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let dir = testing::workspace("threads-sizes", testing::README)?;
    let mut app = app(&dir)?;
    app.view_mut().move_down(2);
    app.view_mut().select_lines();
    app.start_comment();
    type_in(&mut app, "a question about this line");
    app.compose_submit();
    let id = app.marks()[0].id().clone();
    let long_reply = (1..=12)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    testing::external_agent_reply(
        &mut app,
        &id,
        Author::Agent {
            name: "Copilot".to_owned(),
            client: None,
            id: None,
        },
        &long_reply,
        true,
        None,
    )?;

    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    let draw = |app: &mut App, state: &str| -> anyhow::Result<()> {
        for width in [1u16, 2, 4, 8, 12, 20, 40, 80] {
            for height in 1..=6u16 {
                app.resize(usize::from(width), usize::from(height));
                let mut terminal = Terminal::new(TestBackend::new(width, height))?;
                terminal
                    .draw(|frame| crate::app::draw::draw(frame, app, &theme))
                    .with_context(|| format!("{state} at {width}x{height}"))?;
            }
        }
        app.resize(100, 30);
        Ok(())
    };

    draw(&mut app, "text")?;
    app.show_tree();
    draw(&mut app, "sidebar")?;
    app.expand_thread(id);
    draw(&mut app, "thread")?;
    app.thread_reply();
    type_in(&mut app, "a reply long enough to wrap more than once over");
    draw(&mut app, "compose over thread")?;
    app.close_popup();
    app.open_help();
    draw(&mut app, "help")?;
    app.close_popup();
    app.open_status();
    draw(&mut app, "status")?;
    app.close_popup();
    app.open_picker(crate::app::PickerKind::Files);
    app.settle_background();
    draw(&mut app, "picker")?;
    app.close_popup();
    app.open_review();
    draw(&mut app, "review list")?;
    app.thread_reply();
    type_in(&mut app, "a reply from the list");
    draw(&mut app, "compose over list")?;
    Ok(())
}

/// An agent's `resolve` only proposes (ADR 0053): the thread stays
/// resolution-proposed, the status line and review header count it,
/// the entry header uses `◐`, a later plain reply
/// withdraws it, and the user's `o` is what closes the thread.
#[test]
fn a_resolution_proposal_remains_until_superseded_or_resolved() -> anyhow::Result<()> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    let dir = testing::workspace("threads-proposed", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(80, 24);
    app.start_comment();
    type_in(&mut app, "rename this");
    app.compose_submit();
    let id = app.marks()[0].id().clone();
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    let render = |app: &App| -> anyhow::Result<String> {
        let mut terminal = Terminal::new(TestBackend::new(80, 24))?;
        terminal.draw(|frame| crate::app::draw::draw(frame, app, &theme))?;
        let buffer = terminal.backend().buffer().clone();
        Ok((0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n"))
    };

    testing::external_agent_reply(&mut app, &id, Author::agent("bot"), "done", true, None)?;
    let thread = app.thread(&id).context("thread lost")?;
    assert_eq!(thread.status(), Status::Open, "an agent cannot resolve");
    assert!(thread.proposes_resolution());
    assert_eq!(app.proposed_count(), 1);
    assert_eq!(app.proposed_total(), 1);
    assert_eq!(
        app.review_counts(false),
        crate::app::threads::list::Counts {
            active: 0,
            proposed: 1,
            resolved: 0,
        }
    );
    let screen = render(&app)?;
    assert!(screen.contains("1 proposed"), "{screen}");

    app.open_review();
    let rows = app.review_rows(60);
    assert_eq!(rows.entries.len(), 1, "a proposed thread is not hidden");
    assert!(rows.entries[0].proposed());
    assert!(matches!(
        rows.rows.get(1),
        Some(Row::Header { summary, .. })
            if summary.lifecycle() == fathomable_core::annotations::Lifecycle::ResolutionProposed
                && summary.glyph() == "◐"
    ));
    let screen = render(&app)?;
    assert!(
        screen.contains("Threads") && screen.contains("◐ 1 resolution proposed"),
        "a proposal counts under its own circle (ADR 0075): {screen}"
    );
    assert!(
        screen.contains("◐ ▾") && screen.contains("resolve proposed") && screen.contains("L1  ↩1"),
        "{screen}"
    );
    assert!(!screen.contains("waiting"), "{screen}");
    // The keys are on the bar along the list's bottom row (ADR 0059).
    let bar = screen.lines().nth(app.pane_rows() - 1).unwrap_or_default();
    assert!(
        bar.contains("reply c") && bar.contains("auto-resolve R"),
        "{screen}"
    );
    app.close_review();

    // Only the newest reply is read: a plain reply withdraws the proposal.
    testing::external_agent_reply(
        &mut app,
        &id,
        Author::agent("bot"),
        "one more thing",
        false,
        None,
    )?;
    assert_eq!(app.proposed_count(), 0);
    assert_eq!(
        app.review_counts(false),
        crate::app::threads::list::Counts {
            active: 1,
            proposed: 0,
            resolved: 0,
        }
    );
    testing::external_agent_reply(&mut app, &id, Author::agent("bot"), "done now", true, None)?;
    assert_eq!(app.proposed_count(), 1);

    // The user's `o` accepts it.
    app.thread_toggle_resolved();
    assert_eq!(app.message(), Some("resolved"));
    let thread = app.thread(&id).context("thread lost")?;
    assert_eq!(thread.status(), Status::Resolved);
    assert!(!thread.proposes_resolution());
    assert_eq!(app.proposed_count(), 0);
    assert_eq!(
        app.review_counts(false),
        crate::app::threads::list::Counts {
            active: 0,
            proposed: 0,
            resolved: 1,
        }
    );
    Ok(())
}

/// A collapsed header uses the header surface and latest-author colour.
#[test]
fn a_stub_uses_the_header_surface_and_author_colour() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-stub-stripes", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app, "opening")?;
    // A second thread two lines down, whose newest message is an
    // agent's: a stub shows the newest message alone.
    app.view_mut().goto_source_line(5);
    app.view_mut().select_lines();
    app.start_new_comment();
    type_in(&mut app, "second");
    app.compose_submit();
    let id = app.file_threads()[1].clone();
    testing::external_agent_reply(
        &mut app,
        &id,
        Author::agent("reviewer"),
        "agent answer",
        false,
        None,
    )?;
    app.view_mut().goto_top();
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    let buffer = testing::buffer(&app)?;
    let rows = testing::screen(&app)?;
    let gutter = u16::try_from(crate::app::draw::gutter_width(app.view()))?;
    let stub_row = |needle: &str| -> anyhow::Result<u16> {
        let y = rows
            .iter()
            .position(|row| row.contains(needle))
            .with_context(|| format!("the stub row {needle:?}: {rows:?}"))?;
        Ok(u16::try_from(y)?)
    };
    let user_row = stub_row("opening")?;
    let agent_row = stub_row("agent answer")?;
    for (y, kind, name) in [
        (user_row, theme.thread_user, "User"),
        (agent_row, theme.thread_agent, "reviewer"),
    ] {
        let body = &buffer[(gutter + 20, y)];
        assert_eq!(
            body.bg,
            theme.header.bg.unwrap_or_default(),
            "row {y}: {rows:?}"
        );
        let x = u16::try_from(
            rows[usize::from(y)]
                .match_indices(name)
                .next()
                .map(|(byte, _)| rows[usize::from(y)][..byte].chars().count())
                .with_context(|| format!("the name {name:?} on row {y}"))?,
        )?;
        let cell = &buffer[(x, y)];
        assert_eq!(cell.fg, kind.fg.unwrap_or_default(), "row {y}: {rows:?}");
        assert_eq!(
            cell.bg,
            theme.header.bg.unwrap_or_default(),
            "row {y}: {rows:?}"
        );
    }
    Ok(())
}

/// A collapsed stub is one row, the thread's newest message (ADR 0049,
/// amended 2026-09-09), with the thread's circle before the name (ADR
/// 0066).
#[test]
fn a_stub_shows_the_newest_message_and_its_circle() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-stub-one-circle", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app, "opening")?;
    let id = app.marks()[0].id().clone();
    for (body, proposed) in [("first answer", false), ("second answer", true)] {
        testing::external_agent_reply(
            &mut app,
            &id,
            Author::agent("reviewer"),
            body,
            proposed,
            None,
        )?;
    }
    app.view_mut().goto_top();
    let rows = testing::screen(&app)?;
    let stub_row = |needle: &str| -> anyhow::Result<&String> {
        rows.iter()
            .find(|row| row.contains(needle))
            .with_context(|| format!("the stub row {needle:?}: {rows:?}"))
    };
    for older in ["opening", "first answer"] {
        assert!(
            !rows.iter().any(|row| row.contains(older)),
            "{older:?} is not the newest message: {rows:?}"
        );
    }
    let newest = stub_row("second answer")?;
    assert!(
        newest.contains("◐ ▸   reviewer second answer"),
        "{newest:?}"
    );
    Ok(())
}

/// ADR 0071: on an expanded thread's rows the cursor bar is the cursor,
/// so the terminal's is hidden rather than parked on the bar's cell.
#[test]
fn the_terminal_cursor_hides_on_an_expanded_threads_rows() -> anyhow::Result<()> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let dir = testing::workspace("threads-terminal-cursor", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app, "opening")?;
    app.view_mut().goto_top();
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    terminal.draw(|frame| crate::app::draw::draw(frame, &app, &theme))?;
    assert!(terminal.backend().cursor_visible(), "on a source row");
    app.view_mut().move_down(2);
    app.expand_at_cursor();
    anyhow::ensure!(app.shows_thread(), "the thread did not expand");
    // `j` walks into the expanded thread's message rows (ADR 0049).
    let on_thread = |app: &App| {
        app.view()
            .stub_slot_of_row(app.view().cursor().row)
            .is_some()
    };
    for _ in 0..10 {
        if on_thread(&app) {
            break;
        }
        app.view_mut().move_down(1);
    }
    anyhow::ensure!(on_thread(&app), "the text cursor is on the thread's rows");
    terminal.draw(|frame| crate::app::draw::draw(frame, &app, &theme))?;
    assert!(!terminal.backend().cursor_visible(), "on the thread's rows");
    Ok(())
}

/// ADR 0071: in the file the header's bar says which thread the keys
/// act on, so it shows from the thread's lines above; a message's bar
/// waits for the text cursor to be on the thread's own rows.
#[test]
fn the_bar_waits_for_the_text_cursor_to_enter_the_thread() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-bar-waits", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app, "opening")?;
    let id = app.marks()[0].id().clone();
    app.expand_thread(id.clone());
    app.view_mut().goto_top();
    app.thread_step_in_file(1);
    assert_eq!(app.thread_cursor().thread(), Some(&id));
    assert!(
        app.view()
            .stub_slot_of_row(app.view().cursor().row)
            .is_none(),
        "the text cursor is on the thread's first line"
    );
    let gutter = crate::app::draw::gutter_width(app.view());
    let barred = |rows: &[String]| {
        rows.iter()
            .filter(|row| row.chars().nth(gutter) == Some('▎'))
            .count()
    };
    let rows = testing::screen(&app)?;
    assert_eq!(
        barred(&rows),
        1,
        "the header alone from the lines: {rows:?}"
    );
    let header = rows
        .iter()
        .find(|row| row.chars().nth(gutter) == Some('▎'))
        .context("the barred row")?;
    assert!(!header.contains("Resolve"), "the header: {header:?}");
    assert!(
        rows[app.text_bar_row()].contains("resolve r"),
        "{:?}",
        rows[app.text_bar_row()]
    );
    for _ in 0..10 {
        if app
            .view()
            .stub_slot_of_row(app.view().cursor().row)
            .is_some()
        {
            break;
        }
        app.view_mut().move_down(1);
    }
    // The header and the comment's author row and body row.
    assert_eq!(barred(&testing::screen(&app)?), 3, "the bar once inside");
    Ok(())
}

/// A click on a collapsed stub's words rests the text cursor on the
/// stub's row, not the line above (ADR 0073, amended 2026-09-09): the
/// terminal cursor hides, the stub's edge carries the cursor bar, the
/// thread cursor is the stub's thread, and `j` walks on to the next
/// line. A press in the gutter of the row still selects the line it
/// hangs under. The bar is on the thread cursor's stub from its lines
/// above too (ADR 0071).
#[test]
fn a_click_on_a_stub_rests_the_cursor_on_it() -> anyhow::Result<()> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let dir = testing::workspace("threads-click-stub", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app, "opening")?;
    let id = app.marks()[0].id().clone();
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    let bar_fg = theme.thread_cursor.fg.unwrap_or_default();
    let edge_x = app.sidebar_width() + crate::app::draw::gutter_width(app.view());
    let rows = testing::screen(&app)?;
    let stub_y = rows
        .iter()
        .position(|row| row.contains("opening"))
        .with_context(|| format!("the stub row: {rows:?}"))?;
    // From the thread's line the stub is the cursor's: barred, and the
    // block cursor still on the line.
    let buffer = testing::buffer(&app)?;
    let edge = &buffer[(u16::try_from(edge_x)?, u16::try_from(stub_y)?)];
    assert_eq!(
        (edge.symbol(), edge.fg),
        ("▎", bar_fg),
        "barred from the line"
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    terminal.draw(|frame| crate::app::draw::draw(frame, &app, &theme))?;
    assert!(terminal.backend().cursor_visible(), "on the line");

    testing::click(&mut app, edge_x + 10, stub_y);
    let stub_row = app.view().scroll() + stub_y - app.text_top();
    assert_eq!(
        app.view().cursor().row,
        stub_row,
        "the cursor rests on the stub"
    );
    assert_eq!(app.thread_cursor().thread(), Some(&id));
    terminal.draw(|frame| crate::app::draw::draw(frame, &app, &theme))?;
    assert!(
        !terminal.backend().cursor_visible(),
        "no block cursor on a stub"
    );
    let buffer = testing::buffer(&app)?;
    let edge = &buffer[(u16::try_from(edge_x)?, u16::try_from(stub_y)?)];
    assert_eq!(
        (edge.symbol(), edge.fg),
        ("▎", bar_fg),
        "barred on the stub"
    );

    // The paragraph the thread is on renders as one row; `j` steps off
    // the stub onto the row after it, `k` back onto the stub, a stop
    // (ADR 0076), and once more onto the row before it.
    testing::press(&mut app, "j");
    assert_eq!(app.view().cursor().row, stub_row + 1, "j steps off it");
    testing::press(&mut app, "k");
    assert_eq!(app.view().cursor().row, stub_row, "k stops on it");
    testing::press(&mut app, "k");
    assert_eq!(app.view().cursor().row, stub_row - 1, "k steps before it");

    let gutter_x = app.sidebar_width() + 1;
    testing::click(&mut app, gutter_x, stub_y);
    assert_eq!(
        app.view().selected_lines().map(|r| (r.start(), r.end())),
        Some((3, 5)),
        "a gutter press selects the paragraph row the stub hangs under"
    );
    Ok(())
}

/// A click on an expanded thread's header row rests the text cursor on
/// the header, not the line above (ADR 0073, amended 2026-09-10): the
/// terminal cursor hides, the header alone is barred, the thread cursor
/// is the header's thread, and `j` walks on to its first message, `k`
/// back to the line above. A press in the gutter of the row still
/// selects the line it settles on.
#[test]
fn a_click_on_the_header_rests_the_cursor_on_it() -> anyhow::Result<()> {
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let dir = testing::workspace("threads-click-header", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app, "opening")?;
    let id = app.marks()[0].id().clone();
    app.expand_thread(id.clone());
    app.view_mut().goto_top();
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    let gutter = crate::app::draw::gutter_width(app.view());
    let edge_x = app.sidebar_width() + gutter;
    let rows = testing::screen(&app)?;
    let header_y = rows
        .iter()
        .position(|row| row.chars().nth(gutter + 3) == Some('▾'))
        .with_context(|| format!("the header row: {rows:?}"))?;
    let header_row = app.view().scroll() + header_y - app.text_top();
    let barred = |rows: &[String]| {
        rows.iter()
            .filter(|row| row.chars().nth(gutter) == Some('▎'))
            .count()
    };

    testing::click(&mut app, edge_x + 70, header_y);
    assert_eq!(
        app.view().cursor().row,
        header_row,
        "the cursor rests on the header"
    );
    assert_eq!(app.thread_cursor().thread(), Some(&id));
    assert!(app.is_expanded(&id), "one click keeps it open");
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    terminal.draw(|frame| crate::app::draw::draw(frame, &app, &theme))?;
    assert!(
        !terminal.backend().cursor_visible(),
        "no block cursor on the header"
    );
    let rows = testing::screen(&app)?;
    assert_eq!(barred(&rows), 1, "the header alone is barred: {rows:?}");

    testing::press_key(&mut app, KeyCode::Enter);
    assert!(!app.is_expanded(&id), "Enter on the header folds");
    let (stub, index, _) = app
        .stub_on_row(app.view().cursor().row)
        .context("the folded stub under the cursor")?;
    assert_eq!(stub.thread(), Some(&id));
    assert_eq!(index, 0);
    testing::press_key(&mut app, KeyCode::Enter);
    assert!(app.is_expanded(&id), "Enter on the stub unfolds");
    let (stub, index, _) = app
        .stub_on_row(app.view().cursor().row)
        .context("the expanded header under the cursor")?;
    assert_eq!(stub.thread(), Some(&id));
    assert!(stub.expanded());
    assert_eq!(index, 0, "the cursor stays on the header");

    testing::press(&mut app, "j");
    assert_eq!(
        app.expanded_row_message(app.view().cursor().row),
        Some((id.clone(), 0)),
        "j steps onto the first message"
    );
    let message_row = app.view().cursor().row;
    testing::press_key(&mut app, KeyCode::Enter);
    assert!(app.is_expanded(&id), "Enter on a message does nothing");
    assert_eq!(app.view().cursor().row, message_row);
    testing::press(&mut app, "k");
    assert_eq!(app.view().cursor().row, header_row - 1, "k steps over it");
    testing::press_key(&mut app, KeyCode::Enter);
    assert!(app.is_expanded(&id), "Enter on source text does nothing");

    let gutter_x = app.sidebar_width() + 1;
    testing::click(&mut app, gutter_x, header_y);
    assert_eq!(
        app.view().selected_lines().map(|r| (r.start(), r.end())),
        Some((3, 5)),
        "a gutter press selects the paragraph row the header hangs under"
    );
    Ok(())
}

/// Folding the thread the cursor is on, by `z`, a click on the
/// header's chevron, or a double-click on the header, rests the cursor
/// on the thread's stub (ADR 0073, amended 2026-09-10), not at the
/// first column of the line above: the terminal cursor stays hidden,
/// the stub carries the bar, and the view stays still. A cursor resting
/// on a stub stays there while another thread folds.
#[test]
fn folding_the_thread_under_the_cursor_rests_it_on_the_stub() -> anyhow::Result<()> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let dir = testing::workspace("threads-fold-rests", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app, "opening")?;
    let id = app.marks()[0].id().clone();
    app.view_mut().goto_top();
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    let gutter = crate::app::draw::gutter_width(app.view());
    let edge_x = app.sidebar_width() + gutter;
    let header_y = |app: &App| -> anyhow::Result<usize> {
        let rows = testing::screen(app)?;
        rows.iter()
            .position(|row| row.chars().nth(gutter + 3) == Some('▾'))
            .with_context(|| format!("the header row: {rows:?}"))
    };
    let stub_y = |app: &App| -> anyhow::Result<usize> {
        let rows = testing::screen(app)?;
        rows.iter()
            .position(|row| row.chars().nth(gutter + 3) == Some('▸'))
            .with_context(|| format!("the stub row: {rows:?}"))
    };
    let on_stub = |app: &App, y: usize| -> anyhow::Result<()> {
        let stub_row = app.view().scroll() + y - app.text_top();
        anyhow::ensure!(
            app.view().cursor().row == stub_row,
            "the cursor rests on the stub, not row {}",
            app.view().cursor().row
        );
        anyhow::ensure!(app.thread_cursor().thread() == Some(&id));
        let buffer = testing::buffer(app)?;
        let edge = &buffer[(u16::try_from(edge_x)?, u16::try_from(y)?)];
        anyhow::ensure!(edge.symbol() == "▎", "the stub is barred: {edge:?}");
        let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
        terminal.draw(|frame| crate::app::draw::draw(frame, app, &theme))?;
        anyhow::ensure!(!terminal.backend().cursor_visible(), "no block cursor");
        Ok(())
    };

    // `z` from a message row.
    let newest = app.newest_message(&id);
    app.goto_message(id.clone(), newest);
    let y = header_y(&app)?;
    let scroll = app.view().scroll();
    testing::press(&mut app, "z");
    assert!(!app.is_expanded(&id));
    assert_eq!(stub_y(&app)?, y, "the stub takes the header's row");
    assert_eq!(app.view().scroll(), scroll, "the view stays still");
    on_stub(&app, y)?;

    // The chevron, from the header row itself.
    testing::click(&mut app, edge_x + 3, y);
    assert!(app.is_expanded(&id), "the stub's chevron expands");
    testing::click(&mut app, edge_x + 70, y);
    assert_eq!(
        app.view().cursor().row,
        app.view().scroll() + y - app.text_top()
    );
    testing::click(&mut app, edge_x + 3, y);
    assert!(!app.is_expanded(&id), "the header's chevron folds");
    on_stub(&app, y)?;

    // A double-click on the header's words.
    testing::click(&mut app, edge_x + 3, y);
    assert!(app.is_expanded(&id));
    testing::click(&mut app, edge_x + 70, y);
    testing::click(&mut app, edge_x + 70, y);
    assert!(!app.is_expanded(&id), "a double-click folds");
    on_stub(&app, y)?;

    // Another thread folding leaves a cursor resting on a stub where it
    // is.
    app.view_mut().goto_source_line(1);
    app.start_new_comment();
    app.compose_insert("on the first line");
    app.compose_submit();
    let other = app.file_threads()[0].clone();
    assert_ne!(other, id);
    app.goto_message(other.clone(), 0);
    let y = stub_y(&app)?;
    testing::click(&mut app, edge_x + 10, y);
    on_stub(&app, y)?;
    let row = app.view().cursor().row;
    app.fold_thread(&other);
    assert!(
        app.stub_on_row(app.view().cursor().row)
            .is_some_and(|(stub, _, _)| stub.thread() == Some(&id))
    );
    assert!(
        app.view().cursor().row < row,
        "the other thread's rows went"
    );
    Ok(())
}
