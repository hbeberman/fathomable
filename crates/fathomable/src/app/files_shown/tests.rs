use std::fs;
use std::path::Path;

use anyhow::Context as _;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fathomable_core::annotations::{
    Author, Draft, LineRange, OriginSide, OriginVersion, ResolutionOutcome, Store,
};
use fathomable_core::config::DiffMode;
use fathomable_core::theme::Theme as CoreTheme;
use fathomable_core::tree::Rule;
use fathomable_core::workspace::{CommitId, ComparisonEndpoint, Workspace};
use fathomable_testing::{TempDir, git};

use crate::app::draw::Theme;
use crate::app::input::bindings::Where;
use crate::app::input::mouse::handle_mouse;
use crate::app::testing::{self, AppBuilder, buffer, press, screen};
use crate::app::{App, Focus};

/// A repository with a clean `src/lib.rs`, a modified `README.md`,
/// an untracked `notes.txt`, and an ignored `build.log`.
fn fixture(name: &str) -> anyhow::Result<TempDir> {
    let dir = testing::bare(&format!("files-shown-{name}"))?;
    let root = testing::root(&dir);
    git::init(&root)?;
    fs::create_dir_all(root.join("src"))?;
    let committed = [
        ("README.md", "# Readme\n\nhello\n"),
        ("src/lib.rs", "fn lib() {}\n"),
        (".gitignore", "*.log\n"),
    ];
    // The helper writes the objects and the index; the work tree is
    // the test's to write.
    for (path, text) in committed {
        fs::write(root.join(path), text)?;
    }
    git::commit_and_stage(&root, &committed)?;
    fs::write(root.join("README.md"), "# Readme\n\nhello again\n")?;
    fs::write(root.join("notes.txt"), "todo\n")?;
    fs::write(root.join("build.log"), "noise\n")?;
    Ok(dir)
}

fn names(app: &App) -> Vec<String> {
    app.tree()
        .map(|tree| {
            tree.rows()
                .iter()
                .map(|row| row.path().display().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The which-key label under the `Space F` prefix for `key`.
fn label_of(app: &App, key: char) -> anyhow::Result<String> {
    app.which_key(Where::View)
        .into_iter()
        .find(|(chord, _)| chord.to_string() == key.to_string())
        .map(|(_, label)| label)
        .with_context(|| format!("no entry {key}"))
}

fn header_row(app: &App) -> anyhow::Result<String> {
    Ok(screen(app)?.into_iter().next().unwrap_or_default())
}

#[test]
fn the_toggles_filter_the_pane_and_say_what_a_press_does_now() -> anyhow::Result<()> {
    let dir = fixture("toggles")?;
    let mut app = AppBuilder::new(&dir).build()?;
    app.show_tree();
    assert_eq!(
        names(&app),
        ["src", ".gitignore", "notes.txt", "README.md"],
        "the ignored log is hidden and the rest listed"
    );
    let header = header_row(&app)?;
    assert!(header.contains("+2 -1"), "{header}");

    // `Space F` says what each press would do now.
    press(&mut app, " F");
    assert_eq!(label_of(&app, 'c')?, "only changed");
    assert_eq!(label_of(&app, 'o')?, "only reviews");
    assert_eq!(label_of(&app, 'u')?, "hide untracked");
    assert_eq!(label_of(&app, 'i')?, "show ignored");
    press(&mut app, "c");
    assert_eq!(names(&app), ["notes.txt", "README.md"]);
    let header = header_row(&app)?;
    assert!(header.contains("File list"), "{header}");
    assert!(header.contains("c +2 -1"), "{header}");
    assert!(!header.contains('·'), "{header}");

    press(&mut app, " F");
    assert_eq!(label_of(&app, 'c')?, "all files");
    press(&mut app, "u");
    assert_eq!(
        names(&app),
        ["README.md"],
        "untracked dropped from the changed list"
    );
    assert!(header_row(&app)?.contains("c,u +2 -1"));

    // Ignored files are never changed ones: only changed wins.
    press(&mut app, " Fi");
    assert_eq!(names(&app), ["README.md"]);
    // Compact filter state remains readable before the counts.
    let header = header_row(&app)?;
    assert!(header.contains("c,u,i +2 -1"), "{header}");
    app.start_new_comment();
    app.compose_insert("review the readme");
    app.compose_submit();
    press(&mut app, " Fo");
    assert_eq!(names(&app), ["README.md"]);
    app.resize(60, 30);
    let narrow = header_row(&app)?;
    assert!(narrow.contains("+2 -1"), "{narrow}");
    assert!(!narrow.contains("c,r,u,i"), "{narrow}");
    app.resize(100, 30);
    press(&mut app, " Fo");
    press(&mut app, " F");
    assert_eq!(label_of(&app, 'o')?, "only reviews");
    assert_eq!(label_of(&app, 'u')?, "show untracked");
    assert_eq!(label_of(&app, 'i')?, "hide ignored");

    // Back to every file the rules allow: ignored shown, untracked hidden.
    press(&mut app, "c");
    assert_eq!(names(&app), ["src", ".gitignore", "build.log", "README.md"]);
    press(&mut app, " Fu Fi");
    assert_eq!(names(&app), ["src", ".gitignore", "notes.txt", "README.md"]);
    assert!(
        !header_row(&app)?.contains('·'),
        "no marker with no rule on"
    );
    Ok(())
}

#[test]
fn focused_file_list_uses_the_same_filter_suffixes() -> anyhow::Result<()> {
    let dir = fixture("local-toggle-keys")?;
    let mut app = AppBuilder::new(&dir).build()?;
    app.show_tree();
    app.window_files();

    press(&mut app, "c");
    assert_eq!(names(&app), ["notes.txt", "README.md"]);
    press(&mut app, "u");
    assert_eq!(names(&app), ["README.md"]);
    press(&mut app, "i");
    assert_eq!(
        names(&app),
        ["README.md"],
        "ignored paths remain excluded while changed-only is active"
    );
    press(&mut app, "o");
    assert!(names(&app).is_empty());
    Ok(())
}

#[test]
fn file_list_fold_all_works_without_moving_focus() -> anyhow::Result<()> {
    let dir = fixture("remote-fold-all")?;
    let mut app = AppBuilder::new(&dir).build()?;
    app.show_tree();
    app.focus_pane(Focus::View);
    assert_eq!(app.focus(), Focus::View);
    assert!(!names(&app).iter().any(|path| path == "src/lib.rs"));

    press(&mut app, " F");
    assert_eq!(label_of(&app, 'Z')?, "fold or unfold all");
    press(&mut app, "Z");
    app.settle_background();
    assert_eq!(app.focus(), Focus::View);
    assert!(names(&app).iter().any(|path| path == "src/lib.rs"));

    press(&mut app, " FZ");
    app.settle_background();
    assert_eq!(app.focus(), Focus::View);
    assert!(!names(&app).iter().any(|path| path == "src/lib.rs"));
    Ok(())
}

#[test]
fn changed_only_is_dormant_and_restored_around_off() -> anyhow::Result<()> {
    let dir = fixture("changed-off")?;
    let mut app = AppBuilder::new(&dir).build()?;
    app.show_tree();
    app.files_toggle(Rule::Changed);
    assert_eq!(names(&app), ["notes.txt", "README.md"]);

    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    assert!(names(&app).iter().any(|path| path == "src"));
    assert!(app.files_setting_checked(crate::app::input::bindings::Action::FilesChanged));
    assert!(!app.files_shown_marker().contains('c'));
    let off_header = header_row(&app)?;
    assert!(!off_header.contains("+2 -1"), "{off_header}");

    press(&mut app, " F");
    let changed = app
        .which_key(Where::View)
        .into_iter()
        .find(|(chord, _)| chord.to_string() == "c")
        .context("changed-only route")?
        .0;
    assert!(!app.which_key_enabled(Where::View, changed));
    press(&mut app, "c");
    assert_eq!(app.message(), Some("diff mode is off"));
    assert!(app.files_setting_checked(crate::app::input::bindings::Action::FilesChanged));

    app.select_diff_mode(DiffMode::Normal);
    app.settle_background();
    assert_eq!(names(&app), ["notes.txt", "README.md"]);
    assert!(app.files_shown_marker().contains('c'));
    let active_header = header_row(&app)?;
    assert!(active_header.contains("+2 -1"), "{active_header}");
    Ok(())
}

#[test]
fn only_reviews_follows_thread_lifecycle_and_external_reload() -> anyhow::Result<()> {
    let dir = testing::workspace("files-shown-reviews", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.show_tree();
    press(&mut app, " Fo");
    assert!(names(&app).is_empty(), "no open reviews means no rows");
    assert_eq!(
        app.live_label(super::Action::FilesReviews),
        Some("all files")
    );
    assert!(header_row(&app)?.contains(" r"));

    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    app.compose_insert("local finding");
    app.compose_submit();
    assert_eq!(names(&app), ["README.md"], "new thread appears immediately");
    let id = app.file_threads().first().cloned().context("new thread")?;

    assert_eq!(
        testing::external_agent_reply(
            &mut app,
            &id,
            Author::agent("reviewer"),
            "fixed",
            true,
            None,
        )?,
        ResolutionOutcome::ResolutionProposed
    );
    assert_eq!(
        names(&app),
        ["README.md"],
        "resolution-proposed remains open"
    );
    app.toggle_resolved(&id);
    assert!(names(&app).is_empty(), "resolved final review disappears");
    app.toggle_resolved(&id);
    assert_eq!(names(&app), ["README.md"], "reopening restores the file");
    app.delete_thread(&id);
    assert!(
        names(&app).is_empty(),
        "deleting the final review removes it"
    );

    let external = Store::open(testing::store_path(&dir))?.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("README.md"),
            LineRange::new(3, 3),
            "external finding",
        ),
        testing::README,
        10,
    )?;
    assert!(names(&app).is_empty(), "external write is not yet observed");
    assert!(app.reload_store().changed);
    assert_eq!(
        names(&app),
        ["README.md"],
        "store reload refreshes review paths"
    );

    app.toggle_resolved(&external);
    assert!(names(&app).is_empty(), "resolved-only files stay excluded");
    app.archive_thread(&external);
    assert!(names(&app).is_empty(), "archived threads stay excluded");
    app.restore_thread(&external);
    assert!(
        names(&app).is_empty(),
        "restoring does not reopen a resolved thread"
    );
    app.toggle_resolved(&external);
    assert_eq!(names(&app), ["README.md"]);
    Ok(())
}

fn assert_user_message_write_imports_current_head_review(
    name: &str,
    edit: bool,
) -> anyhow::Result<()> {
    let dir = testing::workspace(name, testing::README)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", testing::README)])?;
    let ancestor = Workspace::discover(&root)?
        .head_commit()
        .context("ancestor commit")?;
    let existing = Store::open(testing::store_path(&dir))?.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(3, 3),
            "existing finding",
        )
        .at_source(OriginVersion::commit(ancestor.clone()), OriginSide::Base),
        testing::README,
        1,
    )?;
    fs::write(root.join("b.md"), "# B\n")?;
    git::commit_and_stage(&root, &[("b.md", "# B\n")])?;
    let mut app = testing::source_app(&dir)?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&ancestor)?));
    app.settle_background();
    app.show_tree();
    press(&mut app, " Fo");
    assert_eq!(names(&app), ["README.md"]);
    Store::open(testing::store_path(&dir))?.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("b.md"),
            LineRange::new(1, 1),
            "new at HEAD",
        ),
        "# B\n",
        2,
    )?;

    app.set_thread_cursor_message(existing, 0);
    if edit {
        app.thread_edit_message();
        app.set_compose_text("edited existing finding");
    } else {
        app.thread_reply();
        app.compose_insert("ordinary reply");
    }
    app.compose_submit();

    let rows = names(&app);
    assert_eq!(rows.len(), 2, "write must refresh admitted review rows");
    assert!(
        rows.iter().any(|path| path == "b.md"),
        "the imported current-HEAD review must appear: {rows:?}"
    );
    Ok(())
}

#[test]
fn ordinary_reply_and_edit_import_current_head_reviews() -> anyhow::Result<()> {
    assert_user_message_write_imports_current_head_review("files-shown-reply-import", false)?;
    assert_user_message_write_imports_current_head_review("files-shown-edit-import", true)
}

#[test]
fn rejected_clear_imports_external_resolution_into_files_rows() -> anyhow::Result<()> {
    let dir = testing::workspace("files-shown-clear-rejection", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.start_new_comment();
    app.compose_insert("existing finding");
    app.compose_submit();
    let id = app.file_threads().first().cloned().context("thread")?;
    app.show_tree();
    press(&mut app, " Fo");
    assert_eq!(names(&app), ["README.md"]);

    app.request_clear_board();
    Store::open(testing::store_path(&dir))?.resolve(&id, None, 2)?;
    app.confirm_clear_board();
    assert!(
        matches!(
            app.popup(),
            Some(crate::app::Popup::ConfirmBoard { changed: true, .. })
        ),
        "the stale clear slate is rejected"
    );
    assert!(
        names(&app).is_empty(),
        "the imported resolution removes the last Files row"
    );
    app.cancel_clear_board();
    assert!(
        names(&app).is_empty(),
        "cancelling replacement confirmation keeps the refreshed projection"
    );
    Ok(())
}

#[test]
fn no_op_archive_imports_external_reopen_into_files_rows() -> anyhow::Result<()> {
    let dir = testing::workspace("files-shown-archive-no-op", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.start_new_comment();
    app.compose_insert("existing finding");
    app.compose_submit();
    let id = app.file_threads().first().cloned().context("thread")?;
    app.toggle_resolved(&id);
    app.show_tree();
    press(&mut app, " Fo");
    assert!(names(&app).is_empty());

    Store::open(testing::store_path(&dir))?.reopen(&id, 3)?;
    app.archive_resolved_threads();
    assert_eq!(
        names(&app),
        ["README.md"],
        "the no-op cleanup imports and projects the reopened review"
    );
    Ok(())
}

#[test]
fn only_reviews_reopens_a_thread_resolved_at_a_later_commit() -> anyhow::Result<()> {
    let dir = testing::workspace("files-shown-reopen-commit", testing::README)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", testing::README)])?;
    let mut app = testing::source_app(&dir)?;
    app.show_tree();
    press(&mut app, " Fo");
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    app.compose_insert("finding from commit A");
    app.compose_submit();
    let id = app.file_threads().first().cloned().context("new thread")?;
    assert_eq!(names(&app), ["README.md"]);

    git::commit_and_stage(&root, &[("README.md", testing::README)])?;
    app.toggle_resolved(&id);
    assert!(names(&app).is_empty(), "resolved reviews are excluded");
    app.toggle_resolved(&id);
    assert_eq!(
        names(&app),
        ["README.md"],
        "reach includes the later resolution commit before reopening"
    );
    Ok(())
}

#[test]
fn only_reviews_restores_an_active_archived_thread_after_restart() -> anyhow::Result<()> {
    let dir = testing::workspace("files-shown-restore-restart", testing::README)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", testing::README)])?;
    let commit = Workspace::discover(&root)?
        .head_commit()
        .context("commit A")?;
    let mut store = Store::open(testing::store_path(&dir))?;
    let id = store.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("README.md"),
            LineRange::new(3, 3),
            "archived finding",
        )
        .at_commit(Some(commit)),
        testing::README,
        1,
    )?;
    store.archive(&id, 2)?;
    git::commit_and_stage(&root, &[("README.md", testing::README)])?;

    let mut app = AppBuilder::new(&dir).build()?;
    app.show_tree();
    press(&mut app, " Fo");
    assert!(
        names(&app).is_empty(),
        "archived commits are absent from startup reach"
    );
    app.restore_thread(&id);
    assert!(
        names(&app).is_empty(),
        "restoration does not override exact comparison membership"
    );
    Ok(())
}

#[test]
fn only_reviews_excludes_threads_outside_the_current_reach() -> anyhow::Result<()> {
    let dir = fixture("review-reach")?;
    Store::open(testing::store_path(&dir))?.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("README.md"),
            LineRange::new(1, 1),
            "other branch",
        )
        .at_commit(Some("0123456789abcdef0123456789abcdef01234567".to_owned())),
        "different branch content\n",
        1,
    )?;
    let mut app = AppBuilder::new(&dir).build()?;
    app.show_tree();
    press(&mut app, " Fo");
    assert!(
        names(&app).is_empty(),
        "an off-branch thread does not admit its path"
    );
    Ok(())
}

#[test]
fn only_reviews_follows_a_local_working_tree_rename() -> anyhow::Result<()> {
    let dir = fixture("review-rename")?;
    let root = testing::root(&dir);
    let mut app = AppBuilder::new(&dir).source_view().build()?;
    app.start_new_comment();
    app.compose_insert("rename finding");
    app.compose_submit();
    app.show_tree();
    press(&mut app, " Fo");
    assert_eq!(names(&app), ["README.md"]);

    let from = root.join("README.md");
    let to = root.join("RENAMED.md");
    fs::rename(&from, &to)?;
    app.on_events(vec![crate::app::watch::Event::Renamed { from, to }]);
    app.settle_background();
    assert_eq!(
        names(&app),
        ["RENAMED.md"],
        "viewer-local projection moves the admitted path"
    );
    Ok(())
}

#[test]
fn a_toggle_while_the_pane_is_hidden_names_the_state() -> anyhow::Result<()> {
    let dir = fixture("hidden")?;
    let mut app = AppBuilder::new(&dir).build()?;
    assert_eq!(app.focus(), Focus::View);
    press(&mut app, " Fc");
    assert_eq!(app.message(), Some("File list: changed"));
    press(&mut app, " Fu");
    assert_eq!(app.message(), Some("File list: changed tracked"));
    press(&mut app, " Fc Fu");
    assert_eq!(app.message(), Some("File list: all files"));
    press(&mut app, " Fo");
    assert_eq!(app.message(), Some("File list: reviews"));
    press(&mut app, " Fo");
    // The rules were applied all along: showing the pane lists by them.
    press(&mut app, " Fc");
    app.show_tree();
    assert_eq!(names(&app), ["notes.txt", "README.md"]);
    Ok(())
}

#[test]
fn live_deletion_restoration_keeps_pinned_comparison_entries() -> anyhow::Result<()> {
    use std::path::Path;

    use crate::app::watch::Event;

    let dir = fixture("deleted")?;
    let root = testing::root(&dir);
    let mut app = AppBuilder::new(&dir).build()?;
    let pinned = app.workspace().head_commit().context("pinned HEAD")?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&pinned)?));
    app.settle_background();
    app.show_tree();
    let file = Path::new("src/lib.rs");
    app.with_tree_result(|tree, workspace| tree.reveal(workspace, file).map(|_| None));
    fs::remove_file(root.join(file))?;
    app.on_events(vec![Event::Removed(root.join(file))]);
    app.settle_background();
    assert!(
        names(&app).contains(&"src/lib.rs".to_owned()),
        "rows={:?} comparison={:?}",
        names(&app),
        app.comparison().map(|comparison| comparison
            .changes()
            .iter()
            .map(|change| (change.path().display().to_string(), change.kind()))
            .collect::<Vec<_>>())
    );
    assert_eq!(
        app.tree()
            .and_then(|tree| tree.current())
            .map(fathomable_core::tree::Row::path),
        Some(file)
    );

    // A restored tombstone is already listed, but still needs a disk listing.
    fs::write(root.join(file), "fn lib() {}\n")?;
    app.on_events(vec![Event::Change(root.join(file))]);
    app.settle_background();
    assert!(names(&app).contains(&"src/lib.rs".to_owned()));
    assert!(!app.status().contains(file));
    fs::remove_file(root.join(file))?;
    app.on_events(vec![Event::Removed(root.join(file))]);
    app.settle_background();
    press(&mut app, " Fc Fu");
    assert!(names(&app).contains(&"src/lib.rs".to_owned()));

    git::commit_and_stage(
        &root,
        &[
            ("README.md", "# Readme\n\nhello again\n"),
            (".gitignore", "*.log\n"),
        ],
    )?;
    app.on_events(vec![Event::Change(root.join(".git/index"))]);
    app.settle_background();
    app.settle_status();
    assert!(
        names(&app).contains(&"src/lib.rs".to_owned()),
        "the pinned comparison does not advance with a later commit: {:?}",
        app.comparison().map(|comparison| comparison
            .changes()
            .iter()
            .map(|change| change.path().display().to_string())
            .collect::<Vec<_>>())
    );
    press(&mut app, " Fc Fu");
    assert!(app.comparison().is_some_and(|comparison| {
        comparison
            .changes()
            .iter()
            .any(|change| change.path() == Path::new("src/lib.rs"))
    }));
    Ok(())
}

#[test]
fn the_files_title_opens_checked_settings() -> anyhow::Result<()> {
    let dir = fixture("menu")?;
    let mut app = AppBuilder::new(&dir).build()?;
    app.show_tree();
    let click_title = |app: &mut App| {
        let row = app.pane_top();
        handle_mouse(
            app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 2,
                row: u16::try_from(row).unwrap_or(u16::MAX),
                modifiers: KeyModifiers::NONE,
            },
        )
    };
    click_title(&mut app);
    let grid = app
        .menu()
        .ok_or_else(|| anyhow::anyhow!("Files menu did not open"))?
        .grid_in(app.size().0, app.pane_top(), app.pane_rows());
    assert_eq!(grid.x, 0);
    assert_eq!(grid.y, app.pane_top() + 1);
    let settings = |app: &App| -> Vec<(String, Option<bool>)> {
        app.menu()
            .map(|menu| {
                menu.entries()
                    .iter()
                    .map(|entry| (entry.label().to_owned(), entry.checked()))
                    .collect()
            })
            .unwrap_or_default()
    };
    assert_eq!(
        settings(&app),
        [
            ("only changed".to_owned(), Some(false)),
            ("only reviews".to_owned(), Some(false)),
            ("hide untracked".to_owned(), Some(false)),
            ("show ignored".to_owned(), Some(false)),
        ]
    );
    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: u16::try_from(grid.x + 1)?,
            row: u16::try_from(grid.y + 2)?,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(app.menu().is_none(), "clicking the setting closes the menu");
    assert!(names(&app).is_empty(), "the clicked reviews filter applied");
    press(&mut app, " Fo");
    app.close_popup();
    press(&mut app, " Fc");
    click_title(&mut app);
    assert_eq!(
        settings(&app),
        [
            ("only changed".to_owned(), Some(true)),
            ("only reviews".to_owned(), Some(false)),
            ("hide untracked".to_owned(), Some(false)),
            ("show ignored".to_owned(), Some(false)),
        ]
    );
    app.close_popup();
    press(&mut app, " Fu Fi");
    click_title(&mut app);
    assert_eq!(
        settings(&app),
        [
            ("only changed".to_owned(), Some(true)),
            ("only reviews".to_owned(), Some(false)),
            ("hide untracked".to_owned(), Some(true)),
            ("show ignored".to_owned(), Some(true)),
        ]
    );
    assert!(
        screen(&app)?
            .iter()
            .any(|row| row.contains("✓ hide untracked")),
        "the active filter draws its checkmark"
    );
    app.close_popup();

    let row = app.pane_top();
    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 2,
            row: u16::try_from(row).unwrap_or(u16::MAX),
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(app.menu().is_none(), "right-click leaves the header inert");
    Ok(())
}

#[test]
fn the_files_title_uses_the_shared_hover_background() -> anyhow::Result<()> {
    let dir = fixture("title-hover")?;
    let mut app = AppBuilder::new(&dir).build()?;
    app.show_tree();
    let row = app.pane_top();
    handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: 2,
            row: u16::try_from(row)?,
            modifiers: KeyModifiers::NONE,
        },
    );

    let cells = buffer(&app)?;
    let theme = Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
    for column in 0..11 {
        assert_eq!(
            Some(cells[(column, u16::try_from(row)?)].bg),
            theme.list_hover.bg,
            "File-list title cell {column}"
        );
    }
    assert_eq!(
        Some(cells[(11, u16::try_from(row)?)].bg),
        theme.header.bg,
        "hover stops at the title hit region"
    );
    Ok(())
}
