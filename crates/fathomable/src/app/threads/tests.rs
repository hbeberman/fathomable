use std::fs;
use std::path::{Path, PathBuf};

use fathomable_core::annotations::{LineRange, MessageTarget, Status, Store, Thread};

use fathomable_core::annotations::Author;
use fathomable_core::session::{Request, Response};

use anyhow::Context as _;

use crate::app::Focus;
use fathomable_testing::TempDir;

use crate::app::testing::{self, app};

use fathomable_core::editor::{Cursor, Edit, Motion};

use super::{ComposeTarget, ThreadState};
use crate::app::draw::message::MESSAGE_INDENT;
use crate::app::threads::draft::DraftRow;
use crate::app::threads::list::Row;
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

/// Annotate L3-5 of the open README with `comment`.
fn annotate(app: &mut App, comment: &str) -> anyhow::Result<()> {
    app.view_mut().move_down(2);
    app.view_mut().select_lines();
    app.start_comment();
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
    app.agent_reply(
        &id,
        Author::agent("reviewer"),
        "agent answer".to_owned(),
        false,
        None,
    )
    .map_err(anyhow::Error::msg)?;
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

    // A file rename: the view follows with cursor and marks intact
    // and the store records the move.
    fs::create_dir_all(dir.0.join("ws/docs"))?;
    fs::rename(dir.0.join("ws/README.md"), dir.0.join("ws/docs/GUIDE.md"))?;
    app.on_events(vec![Event::Renamed {
        from: dir.0.join("ws/README.md"),
        to: dir.0.join("ws/docs/GUIDE.md"),
    }]);
    assert_eq!(app.current_path(), Path::new("docs/GUIDE.md"));
    assert_eq!(app.message(), Some("renamed to docs/GUIDE.md"));
    assert_eq!(app.view().cursor(), cursor);
    assert_eq!(app.marks()[0].range(), Some(LineRange::new(3, 5)));
    assert_eq!(app.mark_in(LineRange::new(4, 4)), Some(ThreadState::Open));
    assert_eq!(
        app.thread(&id).map(Thread::path),
        Some(Path::new("docs/GUIDE.md"))
    );
    let store = Store::open(dir.0.join("state/threads.jsonl"))?;
    assert_eq!(
        store.thread(&id).map(Thread::path),
        Some(Path::new("docs/GUIDE.md")),
        "the move is on disk"
    );

    // A directory rename moves everything under it by prefix.
    fs::rename(dir.0.join("ws/docs"), dir.0.join("ws/notes"))?;
    app.on_events(vec![Event::Renamed {
        from: dir.0.join("ws/docs"),
        to: dir.0.join("ws/notes"),
    }]);
    assert_eq!(app.current_path(), Path::new("notes/GUIDE.md"));
    assert_eq!(
        app.thread(&id).map(Thread::path),
        Some(Path::new("notes/GUIDE.md"))
    );
    assert_eq!(app.thread_counts(), (1, 1));

    // A later edit reloads from the new path.
    fs::write(
        dir.0.join("ws/notes/GUIDE.md"),
        "# Readme\n\nintro\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
    )?;
    app.on_events(vec![Event::Change(dir.0.join("ws/notes/GUIDE.md"))]);
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
    assert!(app.deleted());
    assert_eq!(app.banner(), Some("deleted"));
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

    // Shown again while still gone: the file-info pane.
    app.open(Path::new("other.md"));
    assert!(app.info().is_none());
    app.open(Path::new("README.md"));
    let info = app.info().context("no info pane for the deleted file")?;
    assert!(info.rows.iter().any(|(_, v)| v == "deleted"));
    assert_eq!(app.banner(), None);

    // Back on disk: reloaded, banner gone, thread re-anchored.
    fs::write(
        dir.0.join("ws/README.md"),
        "# Readme\n\nintro\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
    )?;
    app.on_events(vec![Event::Created(dir.0.join("ws/README.md"))]);
    assert!(!app.deleted());
    assert!(app.info().is_none());
    assert!(app.view().text().contains("intro"));
    assert_eq!(app.marks()[0].range(), Some(LineRange::new(5, 7)));
    app.start_new_comment();
    assert!(matches!(app.popup(), Some(Popup::Compose(_))));
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
    assert_eq!(app.mark_in(LineRange::new(4, 4)), Some(ThreadState::Open));
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
    assert_eq!(app.mark_in(LineRange::new(6, 6)), Some(ThreadState::Open));
    let reopened = Store::open(dir.0.join("state/threads.jsonl"))?;
    assert!(reopened.threads()[0].edited().is_some());
    assert_eq!(reopened.threads()[0].range(), Some(LineRange::new(5, 7)));

    // The user's reply acknowledges the edit.
    app.view_mut().move_down(3);
    app.expand_at_cursor();
    app.thread_reply();
    type_in(&mut app, "still fine");
    app.compose_submit();
    assert_eq!(app.mark_in(LineRange::new(6, 6)), Some(ThreadState::Open));

    // Rewrite everything: the thread detaches at its last known range.
    fs::write(dir.0.join("ws/README.md"), "# Readme\n\ngone\n")?;
    app.on_changes(vec![dir.0.join("ws/README.md")]);
    assert!(app.marks()[0].is_detached());
    assert_eq!(app.mark_in(LineRange::new(5, 5)), None);
    // Placement and state are told apart (ADR 0032).
    let rows = app.threads_pane_entries();
    assert_eq!(rows[0].words().placement(), Some("detached"));
    assert_eq!(rows[0].words().state(), ThreadState::Open);
    assert_eq!(rows[0].words().glyph(), "?");
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
    app.agent_reply(
        &id,
        Author::Agent {
            name: "Copilot".to_owned(),
            client: Some("github-copilot-developer".to_owned()),
            id: None,
            kind: None,
        },
        long,
        true,
        None,
    )
    .map_err(anyhow::Error::msg)?;
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
            .any(|row| row.contains("waiting · proposed") && !row.contains("fold")),
        "header carries the state:\n{screen}"
    );
    assert!(
        rows[app.text_bar_row()].contains("z fold"),
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
        rows[app.text_bar_row()].contains("Enter submit"),
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
    assert_eq!(app.mark_in(LineRange::new(1, 1)), Some(ThreadState::Open));

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
    app.agent_reply(
        &id,
        Author::agent("reviewer"),
        "agent answer".to_owned(),
        false,
        None,
    )
    .map_err(anyhow::Error::msg)?;
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
    let stub = app
        .stubs()
        .into_iter()
        .next()
        .context("the thread's block")?;
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
    assert_eq!(parts.pill, "NOR");
    assert_eq!(parts.badges, ["SRC"]);
    assert!(
        parts.right_text().contains("1 threads"),
        "{}",
        parts.right_text()
    );
    app.focus_threads_pane();
    let parts = crate::app::draw::status_parts(&app);
    assert_eq!(parts.pill, "THREADS");
    assert_eq!(parts.badges, ["SRC"], "the badge outlives the focus change");
    Ok(())
}

/// Esc leaves a pane where it is; `Space w h` focuses it and `Space w
/// l` hands the keys back, `Space p t` hides it, and `Space r` opens
/// and closes the review list (ADR 0010, ADR 0049, ADR 0056).
#[test]
fn esc_leaves_a_pane_and_its_space_keys_focus_and_hide_it() -> anyhow::Result<()> {
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
    space(&mut app, 'w');
    press(&mut app, KeyCode::Char('h'));
    assert_eq!(
        app.focus(),
        Focus::ThreadsPane,
        "Space w h lands on the pane while the files pane is hidden"
    );
    space(&mut app, 'w');
    press(&mut app, KeyCode::Char('l'));
    assert_eq!(app.focus(), Focus::View, "Space w l hands back");
    assert!(app.threads_pane_shown());
    space(&mut app, 'p');
    press(&mut app, KeyCode::Char('t'));
    assert!(!app.threads_pane_shown(), "Space p t hides it");

    space(&mut app, 'r');
    assert!(app.review_list().is_open());
    assert_eq!(app.focus(), Focus::Review);
    space(&mut app, 'r');
    assert!(
        !app.review_list().is_open(),
        "Space r on the focused list closes it"
    );
    space(&mut app, 'r');
    press(&mut app, KeyCode::Esc);
    assert!(
        !app.review_list().is_open(),
        "Esc closes the list: it is the column"
    );
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

/// ADR 0027: `c` on an annotated row opens the thread, `C` starts a
/// second one there, and `n`/`p` in the pane walk the file's threads
/// in line order with the cursor following.
#[test]
fn c_opens_the_thread_and_n_walks_the_file() -> anyhow::Result<()> {
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

    // `c` again on the row expands the thread in place rather than
    // opening a box (ADR 0049); `c` once more folds it.
    app.start_comment();
    assert!(app.popup().is_none());
    assert_eq!(app.focus(), Focus::View);
    let top = app.thread_cursor().thread().cloned();
    assert!(top.as_ref().is_some_and(|id| app.is_expanded(id)));
    assert_eq!(app.thread_position(), Some((1, 2)));
    app.start_comment();
    assert!(top.as_ref().is_some_and(|id| !app.is_expanded(id)));

    // `C` starts a second thread on the same line.
    app.start_new_comment();
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
    app.review_goto(false);
    for _ in 0..3 {
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
/// entry header and of every row of the cursor's message in the list;
/// in the file the expanded thread's rows begin with a gutter of their
/// own, the bar in its first cell on the header and the cursor's message.
#[test]
fn the_cursor_bar_marks_the_thread_and_its_message_on_both_surfaces() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-cursor-bar", testing::README)?;
    let mut app = app(&dir)?;
    app.resize(100, 30);
    annotate(&mut app, "opening\nsecond line")?;
    let id = app.marks()[0].id().clone();
    app.agent_reply(
        &id,
        Author::agent("reviewer"),
        "agent answer".to_owned(),
        false,
        None,
    )
    .map_err(anyhow::Error::msg)?;
    app.expand_thread(id.clone());
    app.thread_reply();
    type_in(&mut app, "user follow-up");
    app.compose_submit();
    app.open_review();
    assert_eq!(app.thread_cursor().thread(), Some(&id));
    assert_eq!(app.thread_cursor().message(), 2, "the newest message");
    let rows = testing::screen(&app)?;
    let marked: Vec<&String> = rows.iter().filter(|row| row.starts_with('▎')).collect();
    // The entry header, then the newest message's author row and body row.
    assert_eq!(marked.len(), 3, "{rows:?}");
    assert!(
        marked[0].contains("L3") && marked[0].contains("open"),
        "{marked:?}"
    );
    assert!(marked[1].starts_with("▎  User  "), "{marked:?}");
    assert!(marked[2].starts_with("▎    user follow-up"), "{marked:?}");
    let other = rows
        .iter()
        .find(|row| row.contains("agent answer"))
        .context("the agent's body row")?;
    assert!(other.starts_with("     agent answer"), "{other:?}");
    let author = rows
        .iter()
        .find(|row| row.contains("reviewer"))
        .context("the agent's author row")?;
    assert!(author.starts_with("   reviewer  "), "{author:?}");

    // In the file: the global gutter, then the thread's own.
    app.thread_open_in_file();
    let rows = testing::screen(&app)?;
    let gutter = crate::app::draw::gutter_width(app.view());
    let after = |row: &str| row.chars().skip(gutter).collect::<String>();
    let header = rows
        .iter()
        .find(|row| after(row).starts_with("▎▾ ●"))
        .with_context(|| format!("the expanded header: {rows:?}"))?;
    assert!(header.contains("open"), "{header:?}");
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
#[test]
fn the_review_groups_by_file_and_folds_files() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-by-file", testing::README)?;
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
    app.agent_reply(
        &bottom,
        Author::agent("reviewer"),
        "answered".to_owned(),
        false,
        None,
    )
    .map_err(anyhow::Error::msg)?;
    let order = |app: &App| -> Vec<fathomable_core::annotations::ThreadId> {
        app.review_rows(60)
            .entries
            .iter()
            .map(|entry| entry.id().clone())
            .collect()
    };
    let files = |app: &App| -> Vec<String> {
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
    };
    app.open_review();
    assert_eq!(
        order(&app),
        [guide.clone(), top.clone(), bottom.clone()],
        "the directory first, then lines; the reply does not reorder"
    );
    assert_eq!(files(&app), ["docs/guide.md", "README.md"]);

    // `z` on README's first thread folds README; the cursor stays inside
    // and the file row is selected; `k` reaches the guide, `j` comes
    // back to README once, and Enter still opens the file.
    app.set_thread_cursor(top.clone());
    app.review_fold();
    assert_eq!(files(&app), ["docs/guide.md", "▸ README.md"]);
    assert!(
        app.review_rows(60)
            .rows
            .iter()
            .any(|row| matches!(row, Row::File { selected: true, .. }))
    );
    assert!(
        !app.review_rows(60)
            .rows
            .iter()
            .any(|row| matches!(row, Row::Header { entry: 1, .. })),
        "a folded file's threads have no rows"
    );
    app.review_step(-1);
    assert_eq!(app.thread_cursor().thread(), Some(&guide));
    app.review_step(1);
    assert_eq!(app.thread_cursor().thread(), Some(&top));
    app.review_step(1);
    assert_eq!(
        app.thread_cursor().thread(),
        Some(&top),
        "one stop, no further"
    );
    app.review_fold_all();
    assert_eq!(
        files(&app),
        ["docs/guide.md", "README.md"],
        "any folded: unfold all"
    );
    app.review_fold_all();
    assert_eq!(files(&app), ["▸ docs/guide.md", "▸ README.md"]);
    app.review_fold();
    assert_eq!(files(&app), ["▸ docs/guide.md", "README.md"]);
    // `f` lists one file with no file row.
    app.review_toggle_file();
    assert!(files(&app).is_empty());
    assert_eq!(order(&app), [top.clone(), bottom.clone()]);
    Ok(())
}

#[test]
fn socket_requests_open_follow_list_and_reply() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-socket", testing::README)?;
    fs::write(dir.0.join("ws/other.md"), "# Other\n\nline\n")?;
    let mut app = app(&dir)?;
    app.view_mut().move_down(2);
    app.view_mut().select_lines();
    app.start_comment();
    type_in(&mut app, "please check");
    app.compose_submit();
    let id = app.marks()[0].id().clone();

    // Paths must stay inside the workspace.
    for bad in ["../ws/README.md", "/etc/passwd", "missing.md"] {
        let reply = app.handle_request(Request::Open {
            path: PathBuf::from(bad),
            line: None,
            end_line: None,
            worktree: None,
        });
        assert!(matches!(reply, Response::Error(_)), "{bad}: {reply:?}");
    }
    let reply = app.handle_request(Request::Open {
        path: PathBuf::from("other.md"),
        line: Some(3),
        end_line: Some(3),
        worktree: None,
    });
    assert_eq!(reply, Response::Done);
    assert_eq!(app.current_path(), Path::new("other.md"));
    assert_eq!(app.view().cursor_source_line(), Some(3));

    let Response::Threads(all) = app.handle_request(Request::ThreadsList {
        since: None,
        path: None,
    }) else {
        anyhow::bail!("no review list");
    };
    assert_eq!(all.len(), 1);
    let Response::Threads(none) = app.handle_request(Request::ThreadsList {
        since: Some(all[0].updated() + 1),
        path: None,
    }) else {
        anyhow::bail!("no review list");
    };
    assert!(none.is_empty());
    let Response::Threads(elsewhere) = app.handle_request(Request::ThreadsList {
        since: None,
        path: Some(PathBuf::from("other.md")),
    }) else {
        anyhow::bail!("no review list");
    };
    assert!(elsewhere.is_empty());

    let author = Author::Agent {
        name: "reviewer".to_owned(),
        client: Some("claude-code".to_owned()),
        id: None,
        kind: None,
    };
    let reply = app.handle_request(Request::ThreadReply {
        thread: id.clone(),
        author: author.clone(),
        body: "fixed".to_owned(),
        resolve: true,
        lines: None,
    });
    // Answered with the thread as it now stands (ADR 0055).
    let Response::Threads(answered) = reply else {
        anyhow::bail!("reply answered {reply:?}");
    };
    assert_eq!(answered.len(), 1);
    assert_eq!(answered[0].id(), &id);
    assert_eq!(answered[0].replies().len(), 1);
    assert_eq!(
        app.toasts().last().map(crate::app::Toast::text),
        Some("reply on README.md:3, proposes resolving")
    );
    let thread = app
        .thread(&id)
        .ok_or_else(|| anyhow::anyhow!("thread lost"))?;
    // The agent proposed; the thread stays open and waiting (ADR 0053).
    assert_eq!(thread.status(), Status::Open);
    assert!(thread.proposes_resolution());
    assert!(thread.awaits_user());
    assert_eq!(thread.replies()[0].author(), &author);
    assert!(thread.replies()[0].proposes_resolution());
    assert_eq!(
        thread.replies()[0].author().to_string(),
        "reviewer (claude-code)"
    );
    app.open(Path::new("README.md"));
    assert_eq!(app.thread_counts(), (1, 1));
    assert_eq!(app.proposed_count(), 1);
    assert_eq!(app.waiting_count(), 1);

    let reply = app.handle_request(Request::ThreadReply {
        thread: serde_json::from_str(r#""9-9-9""#)?,
        author,
        body: "?".to_owned(),
        resolve: false,
        lines: None,
    });
    assert!(matches!(reply, Response::Error(message) if message.contains("unknown thread")));
    Ok(())
}

/// An agent starts a thread over the socket (ADR 0061): the comment is
/// the agent's, so the thread waits on the user from birth, the viewer
/// toasts it and names the agent on the comment, the user cannot edit
/// that comment, and the cursor stays where it was. A bad path or range
/// is refused.
#[test]
fn socket_requests_start_a_thread() -> anyhow::Result<()> {
    let dir = testing::workspace("threads-socket-start", testing::README)?;
    let mut app = app(&dir)?;
    let before = app.view().cursor_source_line();
    let author = Author::agent("reviewer").subscribed("s-1", "coder");
    let reply = app.handle_request(Request::ThreadStart {
        path: PathBuf::from("README.md"),
        range: Some(LineRange::new(2, 3)),
        author: author.clone(),
        body: "look here".to_owned(),
    });
    let Response::Threads(started) = reply else {
        anyhow::bail!("start answered {reply:?}");
    };
    assert_eq!(started.len(), 1);
    let id = started[0].id().clone();
    assert_eq!(started[0].author(), &author);
    assert_eq!(started[0].comment(), "look here");
    assert_eq!(started[0].range(), Some(LineRange::new(2, 3)));
    assert!(started[0].awaits_user() && !started[0].awaits_agent());
    assert_eq!(
        app.toasts().last().map(crate::app::Toast::text),
        Some("comment on README.md:2 from reviewer (coder)")
    );
    assert_eq!(app.view().cursor_source_line(), before);
    assert_eq!(app.waiting_count(), 1);
    assert_eq!(
        app.message_for(&id, MessageTarget::Comment),
        Some(("look here", false))
    );
    // The review list names the agent on the comment's row as the
    // toast does, and the user's reply by the configured name, the
    // case the comment's row would use (ADR 0058).
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
    assert_eq!(authors, ["reviewer (coder)", "User"], "{:?}", rows.rows);
    app.close_review();

    for (path, range, wrong) in [
        ("missing.md", LineRange::new(1, 1), "cannot read"),
        ("README.md", LineRange::new(400, 401), "past the end"),
    ] {
        let reply = app.handle_request(Request::ThreadStart {
            path: PathBuf::from(path),
            range: Some(range),
            author: author.clone(),
            body: "?".to_owned(),
        });
        assert!(
            matches!(&reply, Response::Error(message) if message.contains(wrong)),
            "{path}: {reply:?}"
        );
    }
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
    // `r` on the list's entry opens the file with the thread expanded
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
    app.agent_reply(
        &id,
        Author::Agent {
            name: "Copilot".to_owned(),
            client: None,
            id: None,
            kind: None,
        },
        (1..=12)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
        true,
        None,
    )
    .map_err(anyhow::Error::msg)?;

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
/// open and waiting, the status line and the review header count it,
/// the entry header reads `waiting · proposed`, a later plain reply
/// withdraws it, and the user's `o` is what closes the thread.
#[test]
fn a_proposal_waits_until_the_user_accepts_it() -> anyhow::Result<()> {
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

    app.agent_reply(&id, Author::agent("bot"), "done".to_owned(), true, None)
        .map_err(anyhow::Error::msg)?;
    let thread = app.thread(&id).context("thread lost")?;
    assert_eq!(thread.status(), Status::Open, "an agent cannot resolve");
    assert!(thread.proposes_resolution());
    assert_eq!(app.proposed_count(), 1);
    assert_eq!(app.proposed_total(), 1);
    assert_eq!(app.waiting_count(), 1, "a proposal is still waiting");
    let screen = render(&app)?;
    assert!(screen.contains("1 proposed  ● 1 waiting"), "{screen}");

    app.open_review();
    let rows = app.review_rows(60);
    assert_eq!(rows.entries.len(), 1, "a proposed thread is not hidden");
    assert!(rows.entries[0].proposed());
    assert!(matches!(
        rows.rows.get(1),
        Some(Row::Header { words, .. }) if words.proposed() && words.glyph() == "◐"
    ));
    let screen = render(&app)?;
    assert!(
        screen.contains("review  ●1"),
        "a proposal counts as waiting: {screen}"
    );
    assert!(screen.contains("◐  L"), "{screen}");
    assert!(screen.contains("waiting · proposed"), "{screen}");
    // The keys are on the bar along the list's bottom row (ADR 0059).
    let bar = screen.lines().nth(app.pane_rows() - 1).unwrap_or_default();
    assert!(bar.contains("x resolved · f file"), "{screen}");
    app.close_review();

    // Only the newest reply is read: a plain reply withdraws the proposal.
    app.agent_reply(
        &id,
        Author::agent("bot"),
        "one more thing".to_owned(),
        false,
        None,
    )
    .map_err(anyhow::Error::msg)?;
    assert_eq!(app.proposed_count(), 0);
    assert_eq!(app.waiting_count(), 1);
    app.agent_reply(&id, Author::agent("bot"), "done now".to_owned(), true, None)
        .map_err(anyhow::Error::msg)?;
    assert_eq!(app.proposed_count(), 1);

    // The user's `o` accepts it.
    app.thread_toggle_resolved();
    assert_eq!(app.message(), Some("resolved"));
    let thread = app.thread(&id).context("thread lost")?;
    assert_eq!(thread.status(), Status::Resolved);
    assert!(!thread.proposes_resolution());
    assert_eq!(app.proposed_count(), 0);
    assert_eq!(app.waiting_count(), 0);
    Ok(())
}

/// A collapsed stub reads as the message does when expanded (ADR 0071):
/// each row on its author's stripe, the name in the author's colour.
#[test]
fn a_stub_takes_its_authors_stripe_and_name_colour() -> anyhow::Result<()> {
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
    app.agent_reply(
        &id,
        Author::agent("reviewer"),
        "agent answer".to_owned(),
        false,
        None,
    )
    .map_err(anyhow::Error::msg)?;
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
        assert_eq!(body.bg, kind.bg.unwrap_or_default(), "row {y}: {rows:?}");
        let x = u16::try_from(
            rows[usize::from(y)]
                .find(name)
                .with_context(|| format!("the name {name:?} on row {y}"))?,
        )?;
        let cell = &buffer[(x, y)];
        assert_eq!(cell.fg, kind.fg.unwrap_or_default(), "row {y}: {rows:?}");
        assert_eq!(cell.bg, kind.bg.unwrap_or_default(), "row {y}: {rows:?}");
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
        app.agent_reply(
            &id,
            Author::agent("reviewer"),
            body.to_owned(),
            proposed,
            None,
        )
        .map_err(anyhow::Error::msg)?;
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
    assert!(newest.contains("▸ ◐ reviewer"), "{newest:?}");
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
    assert!(header.contains("open"), "the header: {header:?}");
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
    // the stub onto the row after it, `k` back over it.
    testing::press(&mut app, "j");
    assert_eq!(app.view().cursor().row, stub_row + 1, "j steps off it");
    testing::press(&mut app, "k");
    assert_eq!(app.view().cursor().row, stub_row - 1, "k steps over it");

    let gutter_x = app.sidebar_width() + 1;
    testing::click(&mut app, gutter_x, stub_y);
    assert_eq!(
        app.view().selected_lines().map(|r| (r.start(), r.end())),
        Some((3, 5)),
        "a gutter press selects the paragraph row the stub hangs under"
    );
    Ok(())
}
