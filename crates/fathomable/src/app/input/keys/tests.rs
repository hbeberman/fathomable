use std::fs;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fathomable_core::annotations::{Author, AutoResolve, Draft, LineRange, MessageTarget, Store};
use fathomable_core::config::DiffMode;
use fathomable_testing::TempDir;

use crate::app::testing::{self, press, source_app};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::handle_key;
use crate::app::input::bindings::Action;
use crate::app::threads::{ComposeTarget, ThreadState};
use crate::app::{App, Focus, Popup};

fn fixture(name: &str) -> std::io::Result<TempDir> {
    let dir = testing::workspace(&format!("keys-{name}"), testing::README)?;
    fs::create_dir_all(dir.0.join("ws/docs"))?;
    fs::write(dir.0.join("ws/docs/guide.md"), "# Guide\n")?;
    Ok(dir)
}

fn annotate(app: &mut App, line: usize, text: &str) {
    app.view_mut().goto_source_line(line);
    app.start_new_comment();
    app.compose_insert(text);
    app.compose_submit();
}

fn alt(app: &mut App, code: KeyCode) {
    handle_key(app, KeyEvent::new(code, KeyModifiers::ALT));
}

fn here(app: &App) -> (String, Option<usize>) {
    (
        app.current_path().to_string_lossy().into_owned(),
        app.view().cursor_source_line(),
    )
}

#[test]
fn off_rejects_comparison_and_git_actions_consistently() -> anyhow::Result<()> {
    let dir = fixture("diff-off-actions")?;
    let mut app = source_app(&dir)?;
    app.select_diff_mode(DiffMode::Off);

    for action in [
        Action::ComparisonWhitespace,
        Action::FilesChanged,
        Action::ChangeNext,
        Action::ChangePrev,
    ] {
        app.act(action);
        assert_eq!(app.message(), Some("diff mode is off"), "{action:?}");
        assert!(app.popup().is_none(), "{action:?} must not open a popup");
    }
    Ok(())
}

fn compose_target(app: &App) -> Option<ComposeTarget> {
    match app.popup() {
        Some(Popup::Compose(compose)) => Some(compose.target().clone()),
        _ => None,
    }
}

fn screen(app: &App) -> anyhow::Result<String> {
    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
    terminal.draw(|frame| crate::app::draw::draw(frame, app, &theme))?;
    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    Ok(text)
}

/// Thread actions follow the Phase C key grammar.
#[test]
fn space_c_acts_on_the_thread_here_from_any_pane() -> anyhow::Result<()> {
    let dir = fixture("threads")?;
    let mut app = source_app(&dir)?;
    annotate(&mut app, 3, "three");
    annotate(&mut app, 5, "five");
    app.view_mut().goto_source_line(3);
    let id = app.marks()[0].id().clone();

    press(&mut app, " cr");
    assert_eq!(compose_target(&app), Some(ComposeTarget::Reply(id.clone())));
    app.compose_insert("answer");
    app.compose_submit();
    assert_eq!(app.thread(&id).map(|t| t.replies().len()), Some(1));
    // A reply expands the thread in place (ADR 0049); focus stays in
    // the text.
    assert_eq!(app.focus(), Focus::View);

    press(&mut app, " ce");
    assert_eq!(
        compose_target(&app),
        Some(ComposeTarget::Edit {
            thread: id.clone(),
            message: MessageTarget::Reply(0),
        }),
        "the newest own message is the reply"
    );
    let draft = match app.popup() {
        Some(Popup::Compose(compose)) => compose.buffer().text().to_owned(),
        _ => String::new(),
    };
    assert_eq!(draft, "answer", "the draft is seeded with the reply");
    app.compose_cancel();

    // Literal thread keys do not steal input or act from the tree.
    app.show_tree();
    assert_eq!(app.focus(), Focus::Tree);
    press(&mut app, "rR");
    assert_eq!(app.marks()[0].kind(), ThreadState::Active);

    // In the text, `R` toggles permission and `r` resolves/reopens.
    app.toggle_tree_focus();
    press(&mut app, "R");
    assert_eq!(
        app.thread(&id)
            .map(fathomable_core::annotations::Thread::auto_resolve),
        Some(AutoResolve::Enabled)
    );
    press(&mut app, "r");
    assert_eq!(app.marks()[0].kind(), ThreadState::Resolved);
    press(&mut app, "R");
    assert_eq!(
        app.thread(&id)
            .map(fathomable_core::annotations::Thread::auto_resolve),
        Some(AutoResolve::Disabled),
        "R is unavailable on a resolved thread"
    );
    press(&mut app, "r");
    assert_eq!(app.marks()[0].kind(), ThreadState::Active);
    press(&mut app, " cd");
    assert_eq!(app.marks().len(), 1);
    assert_eq!(app.marks()[0].range().map(|r| r.start()), Some(5));
    assert_eq!(app.focus(), Focus::View);

    // `Space c c` starts a new thread on the cursor line.
    app.view_mut().goto_source_line(4);
    press(&mut app, " cc");
    assert!(matches!(
        compose_target(&app),
        Some(ComposeTarget::New(range)) if range.start() == 4
    ));
    Ok(())
}

#[test]
fn bare_f_and_t_switch_main_views_and_s_changes_review_scope() -> anyhow::Result<()> {
    let dir = fixture("reviews-open")?;
    let mut app = source_app(&dir)?;
    annotate(&mut app, 3, "three");

    press(&mut app, "t");
    assert!(app.review_list().is_open());
    press(&mut app, "s");
    assert!(app.review().file_only);
    press(&mut app, "f");
    assert!(!app.review_list().is_open());
    assert_eq!(app.focus(), Focus::View);

    press(&mut app, "t");
    app.focus_threads_pane();
    press(&mut app, "t");
    assert!(app.review_list().is_open());
    assert_eq!(app.focus(), Focus::Review);

    app.window_files();
    press(&mut app, "t");
    assert!(app.review_list().is_open());
    press(&mut app, "t");
    assert!(app.review_list().is_open());
    assert_eq!(app.focus(), Focus::Review);

    app.start_new_comment();
    press(&mut app, "tRrwWFT");
    assert_eq!(app.compose_draft(), Some("tRrwWFT"));
    Ok(())
}

#[test]
fn file_list_click_focuses_the_list_without_leaving_threads() -> anyhow::Result<()> {
    let dir = fixture("review-file-paging")?;
    let mut app = source_app(&dir)?;
    app.open(Path::new("docs/guide.md"));
    app.toggle_tree_focus();
    app.open_review();

    let readme = app
        .tree()
        .and_then(|tree| {
            tree.rows()
                .iter()
                .position(|row| row.path() == Path::new("README.md"))
        })
        .ok_or_else(|| anyhow::anyhow!("README.md tree row"))?;
    app.tree_click(readme.saturating_sub(app.tree_scroll()));
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert!(app.review_list().is_open());
    assert_eq!(app.focus(), Focus::Tree);

    press(&mut app, "F");
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        !app.review_list().is_open(),
        "explicit Open enters File view"
    );
    assert_eq!(app.focus(), Focus::View);
    Ok(())
}

#[test]
fn bare_focus_keys_cycle_lists_without_reselecting_content() -> anyhow::Result<()> {
    let dir = fixture("sidebar")?;
    let mut app = source_app(&dir)?;
    app.open(Path::new("docs/guide.md"));
    assert!(app.tree().is_none());

    press(&mut app, " pf");
    assert_eq!(app.focus(), Focus::View, "visibility does not take focus");
    let before = app.tree().map(fathomable_core::tree::Tree::cursor);
    press(&mut app, "F");
    assert_eq!(app.focus(), Focus::Tree);
    assert_eq!(
        app.tree().map(fathomable_core::tree::Tree::cursor),
        before,
        "focus does not reveal or reselect the current file"
    );
    press(&mut app, "F");
    assert_eq!(app.focus(), Focus::Tree, "named focus is idempotent");

    press(&mut app, " pt");
    assert_eq!(app.focus(), Focus::Tree, "visibility keeps existing focus");
    press(&mut app, "w");
    assert_eq!(app.focus(), Focus::ThreadsPane);
    press(&mut app, "w");
    assert_eq!(app.focus(), Focus::View);
    press(&mut app, "w");
    assert_eq!(app.focus(), Focus::Tree);
    press(&mut app, "W");
    assert_eq!(app.focus(), Focus::View);
    press(&mut app, "W");
    assert_eq!(app.focus(), Focus::ThreadsPane);
    press(&mut app, "W");
    assert_eq!(app.focus(), Focus::Tree);

    press(&mut app, "t");
    assert_eq!(app.focus(), Focus::Review);
    press(&mut app, "wW");
    assert_eq!(
        app.focus(),
        Focus::Review,
        "main means the displayed Threads view"
    );

    press(&mut app, " pf");
    assert!(!app.sidebar.tree);
    assert_eq!(app.focus(), Focus::Review);
    press(&mut app, "w");
    assert_eq!(app.focus(), Focus::ThreadsPane, "a hidden list is skipped");
    press(&mut app, "w");
    assert_eq!(app.focus(), Focus::Review);

    press(&mut app, "F");
    assert!(app.sidebar.tree, "F shows the hidden File list");
    assert_eq!(app.focus(), Focus::Tree);
    press(&mut app, " pt");
    assert!(!app.sidebar.threads);
    assert_eq!(app.focus(), Focus::Tree);
    press(&mut app, "T");
    assert!(app.sidebar.threads, "T shows the hidden Thread list");
    assert_eq!(app.focus(), Focus::ThreadsPane);
    press(&mut app, "T");
    assert_eq!(app.focus(), Focus::ThreadsPane, "already there");

    press(&mut app, " ");
    assert!(!app.prefix().is_empty());
    press(&mut app, "w");
    assert!(
        app.prefix().is_empty(),
        "the retired Space w submenu is absent"
    );
    assert_eq!(app.focus(), Focus::ThreadsPane);
    Ok(())
}

#[test]
fn unmatched_second_space_cancels_only_the_prefix() -> anyhow::Result<()> {
    let dir = fixture("cancel-space")?;
    let mut app = source_app(&dir)?;
    app.show_tree();
    app.show_threads_pane();
    app.view_mut().goto_source_line(3);
    app.view_mut().select_chars();
    let cursor = app.view().cursor();
    let selection = app.view().selection();
    for focus in [Focus::View, Focus::Tree, Focus::ThreadsPane, Focus::Review] {
        app.focus = focus;
        press(&mut app, " ");
        assert!(!app.prefix().is_empty());
        press(&mut app, " ");
        assert!(app.prefix().is_empty());
        assert_eq!(app.focus(), focus);
        assert_eq!(app.view().cursor(), cursor);
        assert_eq!(app.view().selection(), selection);
    }
    app.focus = Focus::View;
    app.view_mut().clear_selection();
    app.view_mut().set_bases(None, Some("before\n".to_owned()));
    app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
    press(&mut app, "  ");
    assert!(app.view().diff_view(), "cancel does not act as Escape");
    assert!(app.prefix().is_empty());
    Ok(())
}

/// Far moves leave positions behind: `Tab` across files, `gg`, and a
/// search jump; `Alt-Left` walks back through them and `Alt-Right`
/// forward, a new far move dropping the forward part; `j` and the
/// tree's paging leave nothing (ADR 0049).
#[test]
fn alt_left_and_right_walk_the_positions_far_moves_left() -> anyhow::Result<()> {
    let dir = fixture("jumplist")?;
    let mut app = source_app(&dir)?;
    annotate(&mut app, 3, "readme");
    app.open(Path::new("docs/guide.md"));
    annotate(&mut app, 1, "guide");
    app.open(Path::new("README.md"));
    app.view_mut().goto_source_line(5);

    handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(here(&app), ("docs/guide.md".to_owned(), Some(1)));
    alt(&mut app, KeyCode::Left);
    assert_eq!(here(&app), ("README.md".to_owned(), Some(5)));
    alt(&mut app, KeyCode::Right);
    assert_eq!(here(&app), ("docs/guide.md".to_owned(), Some(1)));
    alt(&mut app, KeyCode::Right);
    assert_eq!(app.message(), Some("at newest position"));

    // Back, then a new far move: the forward part is gone.
    alt(&mut app, KeyCode::Left);
    assert_eq!(here(&app), ("README.md".to_owned(), Some(5)));
    press(&mut app, "jj");
    press(&mut app, "gg");
    assert_eq!(here(&app), ("README.md".to_owned(), Some(1)));
    alt(&mut app, KeyCode::Right);
    assert_eq!(app.message(), Some("at newest position"));
    alt(&mut app, KeyCode::Left);
    assert_eq!(
        here(&app),
        ("README.md".to_owned(), Some(7)),
        "gg left line 7"
    );

    // A search jump records where it left; `j` records nothing, so
    // back from the line below the match returns to the search's
    // origin and forward to where back started.
    press(&mut app, "/beta");
    handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(here(&app), ("README.md".to_owned(), Some(4)));
    press(&mut app, "j");
    alt(&mut app, KeyCode::Left);
    assert_eq!(here(&app), ("README.md".to_owned(), Some(7)));
    alt(&mut app, KeyCode::Right);
    assert_eq!(here(&app), ("README.md".to_owned(), Some(5)));
    Ok(())
}

#[test]
fn tab_review_fallback_round_trips_through_the_jumplist() -> anyhow::Result<()> {
    let dir = testing::workspace("review-jumplist", testing::README)?;
    let thread = Store::open(testing::store_path(&dir))?.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("deleted.md"),
            LineRange::new(1, 1),
            "missing source",
        ),
        "gone\n",
        1,
    )?;
    let mut app = source_app(&dir)?;

    handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(app.review_list().is_open());
    assert_eq!(app.thread_cursor().thread(), Some(&thread));
    let review = testing::screen(&app)?.join("\n");
    assert!(review.contains("origin:"), "{review}");
    assert!(review.contains("excerpt: gone"), "{review}");

    app.thread_reply();
    assert!(
        app.draft().is_none(),
        "fallback must not open a hidden draft"
    );
    assert!(app.review_list().is_open());
    assert_eq!(
        app.message(),
        Some("source is unavailable; reply cannot be placed inline")
    );

    alt(&mut app, KeyCode::Left);
    assert!(!app.review_list().is_open());
    assert_eq!(app.current_path(), Path::new("README.md"));

    alt(&mut app, KeyCode::Right);
    assert!(app.review_list().is_open());
    assert_eq!(app.thread_cursor().thread(), Some(&thread));
    Ok(())
}

#[test]
fn same_file_info_fallback_cannot_open_a_hidden_reply() -> anyhow::Result<()> {
    let dir = testing::workspace("same-file-info-reply", testing::README)?;
    fs::write(testing::root(&dir).join("binary.bin"), [0, 1, 2])?;
    let thread = Store::open(testing::store_path(&dir))?.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("binary.bin"),
            LineRange::new(1, 1),
            "binary source",
        ),
        "source\n",
        1,
    )?;
    let mut app = testing::AppBuilder::new(&dir).unopened().build()?;
    app.open(Path::new("binary.bin"));
    assert!(app.info().is_some());

    handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(app.review_list().is_open());
    assert_eq!(app.thread_cursor().thread(), Some(&thread));
    app.thread_reply();

    assert!(app.draft().is_none());
    assert!(app.review_list().is_open());
    assert_eq!(
        app.message(),
        Some("source is unavailable; reply cannot be placed inline")
    );
    Ok(())
}

#[test]
fn workspace_thread_cycle_excludes_resolved_threads() -> anyhow::Result<()> {
    let dir = fixture("open-thread-cycle")?;
    let mut app = source_app(&dir)?;
    annotate(&mut app, 3, "open");
    annotate(&mut app, 5, "resolved");
    let ids = app.file_threads();
    app.goto_message(ids[1].clone(), 0);
    app.thread_toggle_resolved();

    assert_eq!(app.workspace_threads(), vec![ids[0].clone()]);
    app.view_mut().goto_source_line(1);
    handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.thread_cursor().thread(), Some(&ids[0]));
    Ok(())
}

/// The which-key box leads with the prefix and the submenu's word,
/// and re-renders at every level.
#[test]
fn the_menu_shows_a_breadcrumb_for_the_prefix() -> anyhow::Result<()> {
    let dir = fixture("menu")?;
    let mut app = source_app(&dir)?;
    press(&mut app, " ");
    let text = screen(&app)?;
    assert!(text.contains(" Space "), "the Space menu names its prefix");
    assert!(text.contains("threads…"), "the submenu entry is named");
    press(&mut app, "c");
    let text = screen(&app)?;
    assert!(text.contains(" Space c · threads "), "{text}");
    assert!(text.contains("new thread"), "{text}");
    press(&mut app, "r");
    assert!(matches!(app.popup(), Some(Popup::Compose(_))) || app.message().is_some());
    Ok(())
}
