use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use crossterm::event::KeyCode;
use fathomable_core::annotations::{LineRange, MessageTarget, Store};
use fathomable_core::editor::{Edit, Motion};
use fathomable_core::session::{Request, Response};

use super::ComposeTarget;
use crate::app::App;
use crate::app::input::bindings::Where;
use crate::app::input::keys;
use crate::app::testing::{self, click, press, press_key, screen};

fn click_file(app: &mut App, path: &str) -> anyhow::Result<()> {
    let row = app
        .tree()
        .context("files pane is hidden")?
        .rows()
        .iter()
        .position(|row| row.path() == Path::new(path))
        .context("file missing from tree")?;
    click(app, 2, row + 1);
    assert_eq!(app.current_path(), Path::new(path));
    Ok(())
}

#[test]
fn drafts_stay_in_their_files_when_clicking_away_and_back() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-file-click", testing::README)?;
    fs::write(dir.0.join("ws/main.c"), "first\nsecond\nthird\nfourth\n")?;
    let mut app = testing::source_app(&dir)?;
    app.show_tree();
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    press(&mut app, "README draft");
    app.compose_edit(Edit::Move(Motion::Left));
    let cursor = app.draft().context("README draft")?.buffer().cursor();
    assert!(screen(&app)?.join("\n").contains("README draft"));

    click_file(&mut app, "main.c")?;
    assert!(app.draft().is_none(), "the README draft is not active here");
    assert_eq!(keys::place(&app), Some(Where::Tree));
    assert!(app.draft_cursor_cell().is_none());
    assert!(!screen(&app)?.join("\n").contains("README draft"));
    press_key(&mut app, KeyCode::Enter);
    assert!(Store::open(testing::store_path(&dir))?.threads().is_empty());

    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    press(&mut app, "main draft");
    click_file(&mut app, "README.md")?;
    let draft = app.draft().context("restored README draft")?;
    assert_eq!(draft.buffer().text(), "README draft");
    assert_eq!(draft.buffer().cursor(), cursor);
    assert_eq!(draft.target(), &ComposeTarget::New(LineRange::new(3, 3)));
    assert_eq!(keys::place(&app), Some(Where::Draft));
    let shown = screen(&app)?.join("\n");
    assert!(shown.contains("README draft"));
    assert!(!shown.contains("main draft"));
    press_key(&mut app, KeyCode::Enter);
    assert!(app.draft().is_none());

    click_file(&mut app, "main.c")?;
    assert_eq!(app.compose_draft(), Some("main draft"));
    press_key(&mut app, KeyCode::Enter);
    click_file(&mut app, "README.md")?;
    assert!(app.draft().is_none(), "submitted drafts do not return");
    let store = Store::open(testing::store_path(&dir))?;
    assert_eq!(store.threads().len(), 2);
    let readme = store
        .threads()
        .iter()
        .find(|thread| thread.path() == Path::new("README.md"))
        .context("README thread")?;
    assert_eq!(readme.comment(), "README draft");
    assert_eq!(readme.range(), Some(LineRange::new(3, 3)));
    assert_eq!(readme.snippet(), "alpha");
    let main = store
        .threads()
        .iter()
        .find(|thread| thread.path() == Path::new("main.c"))
        .context("main thread")?;
    assert_eq!(main.comment(), "main draft");
    assert_eq!(main.snippet(), "third");
    Ok(())
}

#[test]
fn file_drafts_survive_agent_navigation_and_cancel_only_in_their_file() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-file-agent", testing::README)?;
    fs::write(dir.0.join("ws/main.c"), "main\n")?;
    let mut app = testing::source_app(&dir)?;
    app.start_file_comment();
    press(&mut app, "whole README");
    app.compose_cancel();
    assert!(app.draft().context("draft")?.confirming_discard());

    for path in ["main.c", "README.md"] {
        assert!(matches!(
            app.handle_request(Request::Open {
                path: PathBuf::from(path),
                line: None,
                end_line: None,
                worktree: None,
            }),
            Response::Done
        ));
        if path == "main.c" {
            assert!(app.draft().is_none());
            app.compose_submit();
            app.start_file_comment();
            press(&mut app, "whole main");
        }
    }
    assert_eq!(app.compose_draft(), Some("whole README"));
    assert!(app.draft().context("restored draft")?.confirming_discard());
    assert!(screen(&app)?.join("\n").contains("comment on README.md"));
    app.compose_cancel();
    app.open(Path::new("main.c"));
    assert_eq!(app.compose_draft(), Some("whole main"));
    app.compose_submit();
    app.open(Path::new("README.md"));
    assert!(app.draft().is_none(), "cancelled draft does not return");

    let store = Store::open(testing::store_path(&dir))?;
    assert_eq!(store.threads().len(), 1);
    assert_eq!(store.threads()[0].path(), Path::new("main.c"));
    assert_eq!(store.threads()[0].range(), None);
    assert_eq!(store.threads()[0].comment(), "whole main");
    Ok(())
}

#[test]
fn reply_and_edit_drafts_resume_in_their_original_thread() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-thread-navigation", testing::README)?;
    fs::write(dir.0.join("ws/main.c"), "main\n")?;
    let mut app = testing::source_app(&dir)?;
    app.start_new_comment();
    press(&mut app, "opening");
    app.compose_submit();
    let id = app.marks()[0].id().clone();

    for target in [
        ComposeTarget::Reply(id.clone()),
        ComposeTarget::Edit {
            thread: id.clone(),
            message: MessageTarget::Comment,
        },
    ] {
        app.open_compose(target.clone());
        app.set_compose_text("changed");
        app.open(Path::new("main.c"));
        assert!(app.draft().is_none());
        assert!(app.draft_cursor_cell().is_none());
        app.compose_submit();
        assert_eq!(app.thread(&id).context("thread")?.comment(), "opening");
        app.open(Path::new("README.md"));
        assert_eq!(app.draft().context("restored draft")?.target(), &target);
        assert_eq!(app.compose_draft(), Some("changed"));
        assert!(app.draft_cursor_cell().is_some());
        assert!(screen(&app)?.join("\n").contains("changed"));
        app.compose_submit();
    }
    let thread = app.thread(&id).context("thread")?;
    assert_eq!(thread.comment(), "changed");
    assert_eq!(thread.replies().len(), 1);
    assert_eq!(thread.replies()[0].body(), "changed");
    Ok(())
}

#[test]
fn reopening_the_same_file_or_failing_to_open_keeps_the_draft() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-open-unchanged", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.start_new_comment();
    press(&mut app, "keep me");
    for path in ["README.md", "missing.c", "README.md"] {
        app.open(Path::new(path));
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.compose_draft(), Some("keep me"));
    }
    app.compose_submit();
    assert_eq!(app.marks().len(), 1);
    Ok(())
}

#[test]
fn starting_a_reply_from_elsewhere_does_not_replace_a_waiting_draft() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-reply-collision", testing::README)?;
    fs::write(dir.0.join("ws/main.c"), "main\n")?;
    let mut app = testing::source_app(&dir)?;
    app.start_new_comment();
    press(&mut app, "opening");
    app.compose_submit();
    let id = app.marks()[0].id().clone();
    app.start_file_comment();
    press(&mut app, "keep this draft");
    app.open(Path::new("main.c"));

    app.open_compose(ComposeTarget::Reply(id));
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.compose_draft(), Some("keep this draft"));
    assert_eq!(
        app.draft().context("original draft")?.target(),
        &ComposeTarget::OnFile
    );
    assert_eq!(
        app.message(),
        Some("finish or discard this file's draft first")
    );
    Ok(())
}

#[test]
fn a_parked_draft_follows_its_documents_rename() -> anyhow::Result<()> {
    use crate::app::watch::Event;

    let dir = testing::workspace("draft-rename", testing::README)?;
    fs::write(dir.0.join("ws/main.c"), "main\n")?;
    let mut app = testing::source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    press(&mut app, "keep with file");
    app.open(Path::new("main.c"));
    let from = dir.0.join("ws/README.md");
    let to = dir.0.join("ws/GUIDE.md");
    fs::rename(&from, &to)?;
    app.on_events(vec![Event::Renamed { from, to }]);
    app.open(Path::new("GUIDE.md"));
    assert_eq!(app.compose_draft(), Some("keep with file"));
    app.compose_submit();

    let store = Store::open(testing::store_path(&dir))?;
    assert_eq!(store.threads().len(), 1);
    assert_eq!(store.threads()[0].path(), Path::new("GUIDE.md"));
    assert_eq!(store.threads()[0].range(), Some(LineRange::new(3, 3)));
    assert_eq!(store.threads()[0].snippet(), "alpha");
    Ok(())
}
