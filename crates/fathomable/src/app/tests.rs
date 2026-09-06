use fathomable_testing::{TempDir, git};

use crate::app::testing::AppBuilder;
use std::fs;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use fathomable_core::config::{JumpConfig, MarkdownConfig, ViewerConfig, WatchConfig};
use fathomable_core::highlight::Highlighter;
use fathomable_core::tree::Tree;
use fathomable_core::workspace::Workspace;

use super::input::bindings::Action;
use super::{App, Focus, Options, PickerKind, Popup};

fn fixture(name: &str) -> std::io::Result<TempDir> {
    let dir = TempDir::new(&format!("app-{name}"))?;
    fs::create_dir_all(dir.0.join("docs"))?;
    fs::write(dir.0.join("README.md"), "# Readme\n\nhello\n")?;
    fs::write(dir.0.join("docs/guide.md"), "# Guide\n")?;
    fs::write(dir.0.join("docs/notes.md"), "# Notes\n")?;
    Ok(dir)
}

fn app(dir: &TempDir) -> anyhow::Result<App> {
    app_with(dir, Options::for_test(dir.0.clone()))
}

/// An `App` on the fixture's root with nothing open, under `options`.
fn app_with(dir: &TempDir, options: Options) -> anyhow::Result<App> {
    AppBuilder::at(&dir.0)
        .unopened()
        .options(|_| options)
        .build()
}

fn picker_items(app: &App) -> Vec<String> {
    match app.popup() {
        Some(Popup::Picker(picker)) => picker
            .matches()
            .iter()
            .map(|m| picker.item(m).to_owned())
            .collect(),
        _ => Vec::new(),
    }
}

#[test]
fn source_files_open_highlighted_and_markdown_files_rendered() -> anyhow::Result<()> {
    let dir = fixture("syntax")?;
    fs::write(dir.0.join("main.rs"), "fn main() {}\n")?;
    fs::write(dir.0.join("LICENSE"), "# Terms\n")?;
    fs::write(dir.0.join("justfile"), "default:\n    make help\n")?;
    let highlighter = Arc::new(Highlighter::new("base16-ocean.dark")?);
    let mut app = AppBuilder::at(&dir.0)
        .unopened()
        .options(|o| Options { highlighter, ..o })
        .build()?;
    app.open(Path::new("main.rs"));
    assert!(app.view().source_view(), "a .rs file opens as source");
    let coloured = app.view().layout().lines()[0]
        .spans()
        .iter()
        .any(|span| span.style().fg.is_some());
    assert!(coloured, "the source layout is highlighted by extension");
    app.open(Path::new("README.md"));
    assert!(!app.view().source_view(), "Markdown opens rendered");
    assert_eq!(app.view().layout().lines()[0].text(), "Readme");
    app.open(Path::new("LICENSE"));
    assert!(
        !app.view().source_view(),
        "listed extensionless names render as Markdown"
    );
    app.open(Path::new("justfile"));
    assert!(
        app.view().source_view(),
        "unlisted extensionless files open as source"
    );

    // A narrower list flips both.
    let mut app = AppBuilder::at(&dir.0)
        .unopened()
        .options(|o| Options {
            markdown: MarkdownConfig {
                extensions: vec!["rs".to_owned()],
                names: Vec::new(),
            },
            ..o
        })
        .build()?;
    app.open(Path::new("LICENSE"));
    assert!(app.view().source_view());
    app.open(Path::new("main.rs"));
    assert!(!app.view().source_view());
    Ok(())
}

#[test]
fn opening_files_builds_the_recent_list() -> anyhow::Result<()> {
    let dir = fixture("history")?;
    let mut app = app(&dir)?;
    assert_eq!(app.current_path(), Path::new(""));
    assert!(
        app.position().is_none(),
        "no position before a file is open"
    );
    app.open(Path::new("README.md"));
    app.open(Path::new("docs/guide.md"));
    assert_eq!(app.current_path(), Path::new("docs/guide.md"));
    app.open(Path::new("missing.md"));
    assert!(app.message().is_some_and(|m| m.contains("missing.md")));
    assert_eq!(app.current_path(), Path::new("docs/guide.md"));

    app.open_picker(PickerKind::Recent);
    assert_eq!(picker_items(&app), ["docs/guide.md", "README.md"]);
    Ok(())
}

/// A sidebar squeezed past the width of its narrowest name still draws
/// whole rows: the git letter has no column to take, and the marks that
/// no longer fit take no width either.
#[test]
fn narrow_sidebar_draws_whole_rows() -> anyhow::Result<()> {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let dir = fixture("narrow")?;
    git::init(&dir.0)?;
    git::commit_and_stage(
        &dir.0,
        &[
            ("README.md", "# Readme\n"),
            (
                "docs/deep/notes.md",
                concat!("# Notes\n", "old\nold\nold\nold\nold\nold\nold\nold\n"),
            ),
        ],
    )?;
    // A modified file earns the git letter, and enough changed lines
    // earn `+n -m` counts wider than the sidebar itself.
    fs::write(dir.0.join("README.md"), "# Readme\n\nmore\n")?;
    fs::create_dir_all(dir.0.join("docs/deep"))?;
    fs::write(
        dir.0.join("docs/deep/notes.md"),
        "# Notes\n".to_owned() + &"line\n".repeat(400),
    )?;
    let mut app = app(&dir)?;
    // Opening the nested file unfolds the tree down to it, so the rows
    // are indented past what a narrow sidebar can show.
    app.open(Path::new("docs/deep/notes.md"));
    app.show_tree();
    assert!(
        app.tree()
            .is_some_and(|tree| tree.rows().iter().any(|row| row.depth() == 2)),
        "the tree is unfolded to the nested file"
    );

    let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
    let theme = crate::app::draw::Theme::from_core(&core);
    for width in 1..=40u16 {
        app.resize(usize::from(width), 12);
        let mut terminal = Terminal::new(TestBackend::new(width, 12))?;
        terminal.draw(|frame| crate::app::draw::draw(frame, &app, &theme))?;
        let buffer = terminal.backend().buffer().clone();
        let divider = u16::try_from(app.sidebar_width())?.saturating_sub(1);
        if divider >= width {
            continue;
        }
        for y in 0..buffer.area.height - 1 {
            assert_eq!(
                buffer[(divider, y)].symbol(),
                "│",
                "row {y} of a {width}-column terminal ends at the divider"
            );
        }
    }
    Ok(())
}

#[test]
fn tree_pane_toggles_focus_and_reveals_current_file() -> anyhow::Result<()> {
    let dir = fixture("sidebar")?;
    let mut app = app(&dir)?;
    assert_eq!(app.sidebar_width(), 0);
    app.open(Path::new("docs/notes.md"));
    app.toggle_tree_focus();
    assert_eq!(app.focus(), Focus::Tree);
    assert_eq!(app.sidebar_width(), 32);
    let selected = app
        .tree()
        .and_then(|tree| tree.current())
        .map(|row| row.path().to_path_buf());
    assert_eq!(selected.as_deref(), Some(Path::new("docs/notes.md")));
    app.toggle_tree_focus();
    assert_eq!(app.focus(), Focus::View);
    assert!(app.tree().is_some(), "tree stays visible");
    app.toggle_tree_shown();
    assert!(app.tree().is_none());
    app.toggle_tree_shown();
    assert!(app.tree().is_some(), "the same key shows it again");
    assert_eq!(app.focus(), Focus::View, "showing does not take the keys");
    Ok(())
}

#[test]
fn ge_goes_to_the_end_like_g_in_the_tree_and_the_view() -> anyhow::Result<()> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{Tree, input::keys};
    let dir = fixture("ge")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    keys::handle_key(&mut app, key('g'));
    keys::handle_key(&mut app, key('e'));
    let bottom = app.view().source_position().0;
    keys::handle_key(&mut app, key('g'));
    keys::handle_key(&mut app, key('g'));
    let top = app.view().source_position().0;
    assert!(bottom > top, "ge leaves the top line");
    keys::handle_key(&mut app, key('G'));
    assert_eq!(app.view().source_position().0, bottom, "ge matches G");

    app.toggle_tree_focus();
    keys::handle_key(&mut app, key('g'));
    keys::handle_key(&mut app, key('g'));
    let top = app
        .tree()
        .and_then(Tree::current)
        .map(|r| r.path().to_path_buf());
    keys::handle_key(&mut app, key('g'));
    keys::handle_key(&mut app, key('e'));
    let last = app
        .tree()
        .and_then(Tree::current)
        .map(|r| r.path().to_path_buf());
    assert_ne!(top, last, "ge leaves the first tree row");
    keys::handle_key(&mut app, key('j'));
    let after = app
        .tree()
        .and_then(Tree::current)
        .map(|r| r.path().to_path_buf());
    assert_eq!(last, after, "ge lands on the last tree row");
    Ok(())
}

#[test]
fn long_lines_wrap_in_rendered_source_and_diff_views() -> anyhow::Result<()> {
    let dir = fixture("wrap")?;
    let long = "abcdefghijklmnopqrstuvwxyz".repeat(8);
    fs::write(dir.0.join("long.md"), format!("```\n{long}\n```\n"))?;
    let mut app = app(&dir)?;
    app.open(Path::new("long.md"));
    app.resize(40, 12);
    for display in 0..3 {
        let layout = app.view().layout();
        assert!(
            layout
                .lines()
                .iter()
                .all(|line| line.width() <= layout.width()),
            "display {display} contains an overlong line"
        );
        match display {
            0 => app.view_mut().toggle_source_view(),
            1 => {
                app.view_mut().set_bases(None, None, Some(String::new()));
                app.view_mut().toggle_head_diff();
            }
            _ => {}
        }
    }
    Ok(())
}

#[test]
fn the_tree_highlight_pages_the_viewer() -> anyhow::Result<()> {
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

    use super::input::keys;
    let dir = fixture("paging")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    app.toggle_tree_focus();
    let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    // Directories come first, so `k` from README.md lands on `docs`.
    keys::handle_key(&mut app, key('k'));
    assert_eq!(
        app.current_path(),
        Path::new("README.md"),
        "a directory row leaves the pane on the file it shows"
    );
    keys::handle_key(&mut app, key('l'));
    keys::handle_key(&mut app, key('j'));
    assert_eq!(app.current_path(), Path::new("docs/guide.md"));
    assert_eq!(app.focus(), Focus::Tree, "paging does not steal focus");

    // The wheel steps one row per tick: guide.md to notes.md, not three
    // rows down.
    crate::app::input::mouse::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 5,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.current_path(), Path::new("docs/notes.md"));
    assert_eq!(app.focus(), Focus::Tree);

    // Paging is browsing, not a far move: the jumplist has nothing.
    app.jump_back();
    assert_eq!(app.message(), Some("at oldest position"));
    assert_eq!(app.current_path(), Path::new("docs/notes.md"));

    // A click pages too: it shows the row it lands on and stays in
    // the tree. Row 0 is the root header, so screen row 4 is README.
    crate::app::input::mouse::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 4,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.focus(), Focus::Tree, "a click does not steal focus");

    // Enter commits: focus moves to the viewer.
    keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.focus(), Focus::View);
    Ok(())
}

#[test]
fn left_at_column_zero_keeps_focus_in_the_text() -> anyhow::Result<()> {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::input::keys;
    let dir = fixture("left")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    app.toggle_tree_focus();
    app.toggle_tree_focus();
    assert_eq!(app.focus(), Focus::View, "the tree is open without focus");
    app.view_mut().move_down(2);
    keys::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
    );
    assert_eq!(
        app.focus(),
        Focus::View,
        "h wraps instead of focusing the tree"
    );
    assert_eq!(app.view().cursor().row, 1);
    Ok(())
}

#[test]
fn picker_filters_and_opens() -> anyhow::Result<()> {
    let dir = fixture("picker")?;
    let mut app = app(&dir)?;
    app.act(Action::PickFile);
    assert_eq!(picker_items(&app).len(), 3);
    for ch in "guide".chars() {
        app.picker_char(ch);
    }
    assert_eq!(picker_items(&app), ["docs/guide.md"]);
    app.picker_confirm();
    assert!(app.popup().is_none());
    assert_eq!(app.current_path(), Path::new("docs/guide.md"));
    assert_eq!(app.focus(), Focus::View);
    Ok(())
}

#[test]
fn unchanged_content_queues_nothing() -> anyhow::Result<()> {
    let dir = fixture("touch")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    app.open(Path::new("docs/guide.md"));
    // A touch (or our own read, reported as a change) on an open file
    // whose text is identical must not hint.
    app.on_changes(vec![dir.0.join("README.md"), dir.0.join("README.md")]);
    assert!(app.queue().is_empty());
    Ok(())
}

/// A thread written against a commit HEAD does not contain is hidden
/// from the marks and from `threads_list`; one written against
/// HEAD, or with no commit, shows. An append by another writer reaches
/// the viewer through the store watch (ADR 0024).
#[test]
fn threads_follow_the_work_and_other_writers_are_picked_up() -> anyhow::Result<()> {
    use fathomable_core::annotations::{Author, Draft, LineRange, Store};
    use fathomable_core::session::{Request, Response};

    let dir = fixture("scope")?;
    git::init(&dir.0)?;
    git::commit_and_stage(&dir.0, &[("README.md", "# Readme\n\nhello\n")])?;
    let workspace = Workspace::discover(&dir.0)?;
    let head = workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
    let store_path = dir.0.join(".state/threads.jsonl");
    let text = "# Readme\n\nhello\n";
    let mut store = Store::open(&store_path)?;
    let here = store.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(1, 1),
            "on this work",
        )
        .at_commit(Some(head)),
        text,
        1,
    )?;
    // Written on lines this checkout does not have, as a thread from
    // another branch is; one whose lines are here would follow HEAD
    // (ADR 0035).
    store.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(2, 2),
            "on other work",
        )
        .at_commit(Some("0123456789abcdef0123456789abcdef01234567".to_owned())),
        "# Readme\nelsewhere\nhello\n",
        2,
    )?;
    let unscoped = store.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(3, 3),
            "unscoped",
        ),
        text,
        3,
    )?;

    let mut app = app_with(
        &dir,
        Options {
            store: Some(Store::open(&store_path)?),
            ..Options::for_test(dir.0.clone())
        },
    )?;
    app.open(Path::new("README.md"));
    let ids: Vec<_> = app.marks().iter().map(|m| m.id().clone()).collect();
    assert_eq!(ids, [here.clone(), unscoped.clone()]);
    let Response::Threads(listed) = app.handle_request(Request::ThreadsList {
        since: None,
        path: None,
    }) else {
        anyhow::bail!("expected threads");
    };
    assert_eq!(listed.len(), 2);

    // Another writer appends while this viewer runs.
    let late = store.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(3, 3),
            "late",
        ),
        text,
        4,
    )?;
    app.on_changes(vec![store_path.clone()]);
    let ids: Vec<_> = app.marks().iter().map(|m| m.id().clone()).collect();
    assert_eq!(ids, [here, unscoped, late]);
    Ok(())
}

/// An amend replaces `HEAD` with a commit that does not descend from
/// it. An open thread whose lines are still in the working tree moves
/// to the new commit and stays visible; one whose lines are gone, and
/// a resolved one, stay scoped to the dropped commit (ADR 0035).
#[test]
fn open_threads_follow_head_across_an_amend() -> anyhow::Result<()> {
    use fathomable_core::annotations::{Author, Draft, LineRange, Store, Thread};

    let dir = fixture("rescope")?;
    git::init(&dir.0)?;
    let text = "# Readme\n\nhello\n";
    git::commit_and_stage(&dir.0, &[("README.md", text)])?;
    fs::write(dir.0.join("README.md"), text)?;
    let workspace = Workspace::discover(&dir.0)?;
    let first = workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
    let store_path = dir.0.join(".state/threads.jsonl");
    let mut store = Store::open(&store_path)?;
    let at = |line: usize, comment: &str| {
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(line, line),
            comment,
        )
        .at_commit(Some(first.clone()))
    };
    let kept = store.annotate(at(1, "kept"), text, 1)?;
    let gone = store.annotate(at(3, "lines gone"), text, 2)?;
    let done = store.annotate(at(1, "resolved"), text, 3)?;
    store.resolve(&done, 4)?;

    let mut app = app_with(
        &dir,
        Options {
            store: Some(Store::open(&store_path)?),
            ..Options::for_test(dir.0.clone())
        },
    )?;
    app.open(Path::new("README.md"));
    assert_eq!(app.marks().len(), 3);

    // Amend: an orphan commit with the third line dropped.
    let amended = "# Readme\n\n";
    git::amend(&dir.0, &[("README.md", amended)])?;
    changed(&mut app, &dir, "README.md", amended)?;
    app.on_changes(vec![dir.0.join(".git/HEAD")]);
    let second = app
        .workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
    assert_ne!(first, second);

    let ids: Vec<_> = app.marks().iter().map(|m| m.id().clone()).collect();
    assert_eq!(ids, std::slice::from_ref(&kept));
    let again = Store::open(&store_path)?;
    assert_eq!(
        again.thread(&kept).and_then(Thread::commit),
        Some(second.as_str())
    );
    assert_eq!(
        again.thread(&gone).and_then(Thread::commit),
        Some(first.as_str())
    );
    assert_eq!(
        again.thread(&done).and_then(Thread::commit),
        Some(first.as_str())
    );
    Ok(())
}

fn changed(app: &mut App, dir: &TempDir, relative: &str, text: &str) -> std::io::Result<()> {
    let absolute = dir.0.join(relative);
    fs::write(&absolute, text)?;
    app.on_changes(vec![absolute]);
    Ok(())
}

#[test]
#[expect(clippy::too_many_lines, reason = "one walk through the whole key set")]
fn hunks_cross_uncommitted_files_in_path_order() -> anyhow::Result<()> {
    use fathomable_core::status::State;

    let dir = fixture("hunks")?;
    git::init(&dir.0)?;
    let committed = [
        ("README.md", "# Readme\n\nhello\n"),
        ("docs/guide.md", "# Guide\n"),
        ("docs/notes.md", "# Notes\n"),
    ];
    git::commit_and_stage(&dir.0, &committed)?;
    fs::write(
        dir.0.join("README.md"),
        "# Readme\n\nfirst\n\nhello\n\nlast\n",
    )?;
    fs::write(dir.0.join("docs/notes.md"), "# Notes\n\nmore\n")?;
    fs::write(dir.0.join("docs/new.md"), "# New\n")?;
    let mut app = app(&dir)?;

    let dirty: Vec<(String, State, bool)> = app
        .status()
        .entries()
        .iter()
        .map(|e| (e.path().display().to_string(), e.state(), e.is_staged()))
        .collect();
    assert_eq!(
        dirty,
        vec![
            ("README.md".to_owned(), State::Modified, false),
            ("docs/new.md".to_owned(), State::Untracked, false),
            ("docs/notes.md".to_owned(), State::Modified, false),
        ]
    );
    assert_eq!(
        app.status()
            .summary_under(Path::new("docs"))
            .map(|d| (d.state, d.added, d.removed)),
        Some((State::Untracked, 3, 0))
    );

    // `]g` walks README's two hunks, then crosses into the next dirty
    // files, then wraps.
    app.open(Path::new("README.md"));
    assert_eq!(app.view().diff_counts(), Some((4, 0)));
    app.hunk_next();
    assert_eq!(app.view().source_position().0, 3);
    app.hunk_next();
    assert_eq!(app.view().source_position().0, 7);
    app.hunk_next();
    assert_eq!(app.current_path(), Path::new("docs/new.md"));
    assert_eq!(app.view().source_position().0, 1);
    assert!(!app.view().line_staged(1), "untracked lines are unstaged");
    app.hunk_next();
    assert_eq!(app.current_path(), Path::new("docs/notes.md"));
    assert_eq!(app.view().source_position().0, 3);
    app.hunk_next();
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.view().source_position().0, 3);
    assert_eq!(app.message(), Some("wrapped to first change"));

    // `[g` from README's first hunk lands on the last hunk of the last
    // dirty file.
    app.hunk_prev();
    assert_eq!(app.current_path(), Path::new("docs/notes.md"));
    assert_eq!(app.view().source_position().0, 3);
    assert_eq!(app.message(), Some("wrapped to last change"));
    app.hunk_prev();
    assert_eq!(app.current_path(), Path::new("docs/new.md"));
    app.hunk_prev();
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert!(
        app.view().source_position().0 >= 6,
        "backwards lands on the last hunk"
    );

    // `]G` / `[G` step by file, always to the first hunk.
    app.dirty_next();
    assert_eq!(app.current_path(), Path::new("docs/new.md"));
    app.dirty_prev();
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.view().source_position().0, 3);
    app.dirty_prev();
    assert_eq!(app.current_path(), Path::new("docs/notes.md"));
    assert_eq!(app.message(), Some("wrapped to last change"));

    // A clean file that is open steps into the next dirty one in
    // path order.
    app.open(Path::new("docs/guide.md"));
    app.hunk_next();
    assert_eq!(app.current_path(), Path::new("docs/new.md"));

    // Staging README marks its lines staged; the index event refreshes
    // both the bases and the dirty set.
    git::stage(
        &dir.0,
        &[
            ("README.md", "# Readme\n\nfirst\n\nhello\n\nlast\n"),
            ("docs/guide.md", "# Guide\n"),
            ("docs/notes.md", "# Notes\n"),
        ],
    )?;
    app.open(Path::new("README.md"));
    app.on_changes(vec![dir.0.join(".git/index")]);
    assert!(app.view().line_staged(3));
    assert!(app.view().line_staged(7));
    assert_eq!(
        app.view().diff_counts(),
        Some((4, 0)),
        "counts stay against HEAD"
    );
    assert_eq!(
        app.status()
            .get(Path::new("README.md"))
            .map(|e| (e.state(), e.is_staged())),
        Some((State::Modified, true))
    );

    // Committing everything empties the set.
    git::commit_and_stage(
        &dir.0,
        &[
            ("README.md", "# Readme\n\nfirst\n\nhello\n\nlast\n"),
            ("docs/guide.md", "# Guide\n"),
            ("docs/new.md", "# New\n"),
            ("docs/notes.md", "# Notes\n\nmore\n"),
        ],
    )?;
    app.on_changes(vec![dir.0.join(".git/HEAD")]);
    assert!(app.status().is_empty());
    assert_eq!(app.view().diff_counts(), Some((0, 0)));
    app.hunk_next();
    assert_eq!(app.message(), Some("nothing uncommitted"));
    Ok(())
}

#[test]
fn workspace_changes_queue_newest_first_and_jump() -> anyhow::Result<()> {
    let dir = fixture("changes")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    assert!(app.queue().is_empty());

    changed(&mut app, &dir, "docs/guide.md", "# Guide\n\nmore\n")?;
    changed(&mut app, &dir, "docs/notes.md", "# Notes\n\nnew\n")?;
    assert_eq!(app.queue().len(), 2);
    assert_eq!(
        app.queue().newest().map(|c| c.path.clone()),
        Some(PathBuf::from("docs/notes.md"))
    );
    assert!(app.has_change_under(Path::new("docs")));
    assert!(!app.has_change_under(Path::new("README.md")));
    assert_eq!(app.toasts().len(), 2);

    app.jump_newest();
    assert_eq!(app.current_path(), Path::new("docs/notes.md"));
    assert_eq!(app.queue().len(), 1, "a visited change leaves the queue");
    app.jump_next();
    assert_eq!(app.current_path(), Path::new("docs/guide.md"));
    assert!(app.queue().is_empty());
    app.jump_next();
    assert_eq!(app.message(), Some("no changes"));

    // A change to the open file reloads it and lands on the first hunk.
    changed(
        &mut app,
        &dir,
        "docs/guide.md",
        "# Guide\n\nmore\n\nagain\n",
    )?;
    assert!(app.view().text().contains("again"));
    assert_eq!(app.queue().newest().map(|c| c.target.line()), Some(4));
    app.jump_newest();
    // The blank line 4 has no rendered row; the cursor lands on the
    // next one, as `]c` does.
    assert!((4..=5).contains(&app.view().source_position().0));
    Ok(())
}

#[test]
fn watcher_events_refresh_the_listing_they_land_in() -> anyhow::Result<()> {
    use super::watch::Event;
    let dir = fixture("tree-events")?;
    let watch = WatchConfig {
        ignore: vec!["build/**".to_owned()],
        ..WatchConfig::default()
    };
    let mut app = AppBuilder::at(&dir.0)
        .unopened()
        .options(|o| Options { watch, ..o })
        .build()?;
    app.toggle_tree_focus();
    app.open_picker(PickerKind::Files);
    assert!(!picker_items(&app).iter().any(|p| p == "NEW.md"));
    app.close_popup();
    let has = |app: &App, path: &str| app.tree().is_some_and(|t| t.contains(Path::new(path)));

    // A created file lands in the root listing and the picker index.
    fs::write(dir.0.join("NEW.md"), "# New\n")?;
    app.on_events(vec![Event::Created(dir.0.join("NEW.md"))]);
    assert!(has(&app, "NEW.md"));
    assert_eq!(app.tree().map(Tree::cursor), Some(0), "cursor stays");
    app.open_picker(PickerKind::Files);
    assert!(picker_items(&app).iter().any(|p| p == "NEW.md"));
    app.close_popup();

    // If the platform says it lost events, a full rescan discovers a
    // creation whose individual event never arrived.
    fs::write(dir.0.join("MISSED.md"), "# Missed\n")?;
    app.on_events(vec![Event::Rescan]);
    assert!(has(&app, "MISSED.md"));
    app.open_picker(PickerKind::Files);
    assert!(picker_items(&app).iter().any(|p| p == "MISSED.md"));
    app.close_popup();

    // A collapsed directory is not re-read until it is expanded.
    fs::write(dir.0.join("docs/deep.md"), "# Deep\n")?;
    app.on_events(vec![Event::Created(dir.0.join("docs/deep.md"))]);
    assert!(!has(&app, "docs/deep.md"));
    app.with_tree_result(Tree::activate);
    assert!(has(&app, "docs/deep.md"));

    // A file written into a directory the tree has never listed —
    // an agent making a crate and filling it in one burst — brings
    // that directory into view; what is inside it waits for the
    // expansion.
    fs::create_dir_all(dir.0.join("crates/pipe/src"))?;
    fs::write(dir.0.join("crates/pipe/Cargo.toml"), "[package]\n")?;
    app.on_events(vec![Event::Created(dir.0.join("crates/pipe/Cargo.toml"))]);
    assert!(has(&app, "crates"));
    assert!(!has(&app, "crates/pipe"), "the new listing stays lazy");

    // A path `follow.ignore` hides never triggers a re-read.
    fs::create_dir_all(dir.0.join("build"))?;
    fs::write(dir.0.join("build/out"), "")?;
    app.on_events(vec![Event::Created(dir.0.join("build/out"))]);
    assert!(!has(&app, "build"));

    // A rename re-reads both listings.
    fs::rename(dir.0.join("NEW.md"), dir.0.join("docs/MOVED.md"))?;
    app.on_events(vec![Event::Renamed {
        from: dir.0.join("NEW.md"),
        to: dir.0.join("docs/MOVED.md"),
    }]);
    assert!(!has(&app, "NEW.md"));
    assert!(has(&app, "docs/MOVED.md"));
    Ok(())
}

#[test]
fn new_and_removed_files_update_the_tree() -> anyhow::Result<()> {
    let dir = fixture("tree-watch")?;
    let mut app = app(&dir)?;
    app.toggle_tree_focus();
    let names = |app: &App| -> Vec<String> {
        app.tree()
            .map(|tree| {
                tree.rows()
                    .iter()
                    .map(|row| row.name().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    };
    assert!(!names(&app).contains(&"NEW.md".to_owned()));

    changed(&mut app, &dir, "NEW.md", "# New\n")?;
    assert!(
        names(&app).contains(&"NEW.md".to_owned()),
        "created file shows up"
    );

    let absolute = dir.0.join("NEW.md");
    fs::remove_file(&absolute)?;
    app.on_changes(vec![absolute]);
    assert!(
        !names(&app).contains(&"NEW.md".to_owned()),
        "removed file goes away"
    );
    Ok(())
}

#[test]
fn a_file_event_refreshes_the_dirty_set_for_its_path_only() -> anyhow::Result<()> {
    use fathomable_core::status::State;

    use super::watch::Event;

    let dir = fixture("status-events")?;
    git::init(&dir.0)?;
    git::commit_and_stage(
        &dir.0,
        &[
            ("README.md", "# Readme\n\nhello\n"),
            ("docs/guide.md", "# Guide\n"),
            ("docs/notes.md", "# Notes\n"),
        ],
    )?;
    let mut app = app(&dir)?;
    assert!(app.status().is_empty());

    // Two edits, one event: the set shows the path the event named and
    // nothing else was looked at.
    fs::write(dir.0.join("docs/guide.md"), "# Guide\n\nmore\n")?;
    changed(&mut app, &dir, "README.md", "# Readme\n\nhello\n\nbye\n")?;
    let dirty = |app: &App| -> Vec<(String, State)> {
        app.status()
            .entries()
            .iter()
            .map(|e| (e.path().display().to_string(), e.state()))
            .collect()
    };
    assert_eq!(dirty(&app), vec![("README.md".to_owned(), State::Modified)]);

    // Lost events walk the whole tree again.
    app.on_events(vec![Event::Rescan]);
    assert_eq!(
        dirty(&app),
        vec![
            ("README.md".to_owned(), State::Modified),
            ("docs/guide.md".to_owned(), State::Modified),
        ]
    );

    // A directory renamed away and back: both sides of the rename are
    // examined.
    fs::rename(dir.0.join("docs"), dir.0.join("moved"))?;
    app.on_events(vec![Event::Renamed {
        from: dir.0.join("docs"),
        to: dir.0.join("moved"),
    }]);
    assert_eq!(
        dirty(&app),
        vec![
            ("README.md".to_owned(), State::Modified),
            ("docs/guide.md".to_owned(), State::Deleted),
            ("docs/notes.md".to_owned(), State::Deleted),
            ("moved/guide.md".to_owned(), State::Untracked),
            ("moved/notes.md".to_owned(), State::Untracked),
        ]
    );
    Ok(())
}

#[test]
fn an_ignore_file_event_reloads_the_rules() -> anyhow::Result<()> {
    let dir = fixture("rules-watch")?;
    git::init(&dir.0)?;
    git::commit_and_stage(&dir.0, &[("README.md", "# Readme\n\nhello\n")])?;
    let mut app = app(&dir)?;
    app.toggle_tree_focus();
    let has = |app: &App, path: &str| app.tree().is_some_and(|t| t.contains(Path::new(path)));
    assert!(has(&app, "docs"));
    assert!(app.status().contains(Path::new("docs/guide.md")));

    // The rules file's own event is enough: the tree and the dirty set
    // stop showing what it now ignores.
    changed(&mut app, &dir, ".gitignore", "docs/\n")?;
    assert!(!has(&app, "docs"), "the listing follows the new rule");
    assert!(
        !app.status().contains(Path::new("docs/guide.md")),
        "the dirty set follows it too"
    );
    assert!(app.status().contains(Path::new(".gitignore")));
    Ok(())
}

#[test]
fn ignore_rules_filter_hints_but_not_reloads() -> anyhow::Result<()> {
    let dir = fixture("source")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    changed(&mut app, &dir, "README.md", "# Readme\n\nchanged\n")?;
    assert_eq!(app.queue().len(), 1, "an edit to the open file hints");
    assert!(
        app.view().text().contains("changed"),
        "and the open file reloads"
    );

    let watch = WatchConfig {
        ignore: vec!["docs/**".to_owned()],
        ..WatchConfig::default()
    };
    let jump = JumpConfig {
        toast: std::time::Duration::ZERO,
        ..JumpConfig::default()
    };
    let mut app = app_with(
        &dir,
        Options {
            jump,
            watch,
            ..Options::for_test(dir.0.clone())
        },
    )?;
    changed(&mut app, &dir, "docs/guide.md", "# Guide\n\n3\n")?;
    assert!(app.queue().is_empty(), "watch.ignore globs apply");
    changed(&mut app, &dir, "README.md", "# Readme\n\nx\n")?;
    assert_eq!(app.queue().len(), 1);
    assert!(app.toasts().is_empty(), "toast 0 disables toasts");

    app.command("auto");
    assert!(app.auto_jump());
    app.command("auto off");
    assert!(!app.auto_jump());
    app.command("status");
    assert!(matches!(app.popup(), Some(Popup::Status)));
    app.close_popup();
    app.command("nonsense");
    assert!(
        app.message()
            .is_some_and(|m| m.starts_with("not a command"))
    );
    Ok(())
}

/// The added lines of the last-seen diff view, or `None` when the
/// view cannot show one.
fn seen_diff_added(app: &mut App) -> Option<Vec<String>> {
    app.toggle_seen_diff();
    if app.view().diff_base() != Some(&crate::app::diff::Side::Seen) {
        return None;
    }
    let added = app
        .view()
        .layout()
        .lines()
        .iter()
        .map(fathomable_core::layout::Line::text)
        .filter(|t| t.starts_with('+'))
        .collect();
    app.toggle_seen_diff();
    Some(added)
}

#[test]
fn seen_snapshots_feed_the_seen_diff_view() -> anyhow::Result<()> {
    let dir = fixture("seen")?;
    let seen_store = || fathomable_core::seen::Store::open(&dir.0.join(".seen-state"));
    let mut app = app_with(
        &dir,
        Options {
            seen: Some(seen_store()?),
            ..Options::for_test(dir.0.clone())
        },
    )?;
    app.open(Path::new("README.md"));
    assert_eq!(app.view().diff_counts(), None, "not in git: no gutter");
    assert_eq!(seen_diff_added(&mut app), None, "never seen: no seen diff");

    // Switching away snapshots the file; coming back, the seen diff
    // view shows what arrived, while the gutter stays git-only.
    app.open(Path::new("docs/guide.md"));
    fs::write(dir.0.join("README.md"), "# Readme\n\nhello\n\nworld\n")?;
    app.on_changes(vec![dir.0.join("README.md")]);
    assert_eq!(
        app.queue().newest().map(|c| c.target.line()),
        Some(4),
        "outside git an unopened file's target comes from its last-seen base"
    );
    app.open(Path::new("README.md"));
    assert_eq!(app.view().diff_counts(), None, "the gutter means git");
    assert_eq!(
        seen_diff_added(&mut app).as_deref(),
        Some(&["+".to_owned(), "+world".to_owned()][..])
    );

    // Idle long enough, the tick marks it seen and the next change is
    // measured from there.
    app.settle();
    assert!(
        app.queue().is_empty(),
        "target on screen settles the change"
    );
    std::thread::sleep(std::time::Duration::from_millis(20));
    let idle = ViewerConfig {
        seen_idle: std::time::Duration::from_millis(1),
        ..ViewerConfig::default()
    };
    let mut app2 = app_with(
        &dir,
        Options {
            viewer: idle,
            seen: Some(seen_store()?),
            ..Options::for_test(dir.0.clone())
        },
    )?;
    app2.open(Path::new("README.md"));
    assert_eq!(seen_diff_added(&mut app2).map(|a| a.len()), Some(2));
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(app2.tick_in().is_some());
    app2.tick();
    fs::write(dir.0.join("README.md"), "# Readme\n\nhello\n\nworld\n\n!\n")?;
    app2.on_changes(vec![dir.0.join("README.md")]);
    assert_eq!(
        seen_diff_added(&mut app2).as_deref(),
        Some(&["+".to_owned(), "+!".to_owned()][..]),
        "base is the idle snapshot"
    );
    app2.on_quit();
    Ok(())
}

#[test]
fn auto_jump_waits_for_quiet_and_guardrails() -> anyhow::Result<()> {
    let dir = fixture("auto")?;
    let jump = JumpConfig {
        auto: true,
        debounce: std::time::Duration::ZERO,
        ..JumpConfig::default()
    };
    let mut app = app_with(
        &dir,
        Options {
            jump,
            ..Options::for_test(dir.0.clone())
        },
    )?;
    app.open(Path::new("README.md"));
    changed(&mut app, &dir, "docs/notes.md", "# Notes\n\nnew\n")?;
    assert!(app.tick_in().is_some());
    app.tick();
    assert_eq!(
        app.current_path(),
        Path::new("README.md"),
        "the reader just opened a file: recent activity holds the jump"
    );

    // No activity in the welcome view: nothing open, so the jump goes.
    let jump = JumpConfig {
        auto: true,
        debounce: std::time::Duration::ZERO,
        ..JumpConfig::default()
    };
    let mut app = app_with(
        &dir,
        Options {
            jump,
            ..Options::for_test(dir.0.clone())
        },
    )?;
    changed(&mut app, &dir, "docs/notes.md", "# Notes\n\nnewer\n")?;
    app.tick();
    assert_eq!(app.current_path(), Path::new("docs/notes.md"));
    assert!(app.queue().is_empty());

    let key = |c| {
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(c),
            crossterm::event::KeyModifiers::NONE,
        )
    };
    super::input::keys::handle_key(&mut app, key(' '));
    super::input::keys::handle_key(&mut app, key('j'));
    assert_eq!(super::input::bindings::spell(app.prefix()), "Space j");
    super::input::keys::handle_key(&mut app, key('a'));
    assert!(!app.auto_jump());
    assert!(app.prefix().is_empty());
    Ok(())
}

#[test]
fn agent_open_queues_a_settled_range() -> anyhow::Result<()> {
    use fathomable_core::session::{Request, Response};

    let dir = fixture("agent")?;
    let mut app = app(&dir)?;
    let response = app.handle_request(Request::Open {
        path: PathBuf::from("README.md"),
        line: Some(1),
        end_line: Some(3),
    });
    assert_eq!(response, Response::Done);
    assert_eq!(app.queue().len(), 1);
    app.settle();
    assert!(app.queue().is_empty(), "the opened range is on screen");
    Ok(())
}

/// An agent's range is brought on screen, not selected: the reader
/// is left in normal mode at its first line (ADR 0014, amended).
#[test]
fn agent_open_range_shows_without_selecting() -> anyhow::Result<()> {
    use fathomable_core::session::{Request, Response};

    let dir = fixture("agent-range")?;
    let body = "line\n".repeat(60);
    fs::write(dir.0.join("long.txt"), body)?;
    let mut app = app(&dir)?;
    app.resize(80, 12);
    let response = app.handle_request(Request::Open {
        path: PathBuf::from("long.txt"),
        line: Some(30),
        end_line: Some(36),
    });
    assert_eq!(response, Response::Done);
    assert_eq!(app.view().mode(), super::view::Mode::Normal);
    assert!(app.view().selection().is_none(), "nothing is selected");
    assert_eq!(app.view().cursor_source_line(), Some(30));
    assert!(app.view().line_on_screen(30));
    assert!(
        app.view().line_on_screen(36),
        "the end of the range is on screen"
    );

    // A range longer than the screen keeps its start visible.
    app.handle_request(Request::Open {
        path: PathBuf::from("long.txt"),
        line: Some(10),
        end_line: Some(60),
    });
    assert_eq!(app.view().cursor_source_line(), Some(10));
    assert!(app.view().line_on_screen(10));
    assert!(app.view().selection().is_none());
    Ok(())
}

#[test]
fn binary_and_oversized_files_open_as_file_info() -> anyhow::Result<()> {
    use fathomable_core::config::ViewerConfig;

    let dir = fixture("binary")?;
    fs::write(dir.0.join("plugin.wasm"), b"\0asm\x01\0\0\0")?;
    fs::write(dir.0.join("big.log"), "x".repeat(3 * 1024 * 1024))?;
    let mut app = app_with(
        &dir,
        Options {
            viewer: ViewerConfig {
                max_file_size_mib: 2,
                ..ViewerConfig::default()
            },
            config_path: PathBuf::from("/etc/fathomable/config.kdl"),
            ..Options::for_test(dir.0.clone())
        },
    )?;

    app.open(Path::new("plugin.wasm"));
    assert_eq!(app.current_path(), Path::new("plugin.wasm"));
    let info = app
        .info()
        .ok_or_else(|| anyhow::anyhow!("a binary opens as file info"))?;
    let rows: Vec<(&str, &str)> = info
        .rows
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(rows[0], ("format", "WebAssembly module"));
    assert_eq!(rows[1], ("size", "8 B"));
    assert_eq!(rows[2], ("mode", "regular file"));
    assert_eq!(rows.last(), Some(&("git", "no repository")));
    assert_eq!(app.view().text(), "");
    app.start_comment();
    assert_eq!(app.message(), Some("cannot annotate a binary file"));
    assert!(app.popup().is_none());

    app.open(Path::new("big.log"));
    let info = app
        .info()
        .ok_or_else(|| anyhow::anyhow!("an oversized file opens as file info"))?;
    assert_eq!(info.rows[0].1, "text, too large to view");
    assert_eq!(
        info.notice,
        [
            "Too large to view: 3 MiB, limit is 2 MiB.",
            "Raise it with `viewer { max-file-size-mib 4 }` in /etc/fathomable/config.kdl",
        ]
    );
    app.start_comment();
    assert_eq!(app.message(), Some("cannot annotate a file this large"));

    // Text files are unaffected and the jumplist spans both kinds.
    app.record_jump_from_here();
    app.open(Path::new("README.md"));
    assert!(app.info().is_none());
    app.jump_back();
    assert_eq!(app.current_path(), Path::new("big.log"));
    assert!(app.info().is_some());
    Ok(())
}
