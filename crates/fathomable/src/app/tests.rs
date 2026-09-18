use fathomable_testing::{TempDir, git};

use crate::app::testing::{AppBuilder, complete_highlights, press};
use std::fs;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use fathomable_core::annotations::Store;
use fathomable_core::config::{DiffMode, MarkdownConfig, SidebarConfig, ViewerConfig, WatchConfig};
use fathomable_core::highlight::Highlighter;
use fathomable_core::tree::Tree;
use fathomable_core::workspace::Workspace;

use super::input::bindings::Action;
use super::{App, Focus, NoticeTone, Options, PickerKind, PickerState, Popup};

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

#[test]
fn configured_layout_is_consistent_for_file_and_workspace_starts() -> anyhow::Result<()> {
    let dir = fixture("layout-start")?;
    let make = || -> anyhow::Result<App> {
        let workspace = Workspace::discover(&dir.0)?;
        Ok(App::new(
            workspace,
            100,
            30,
            Options {
                menu_bar: true,
                sidebar: SidebarConfig::default(),
                ..Options::for_test(dir.0.clone())
            },
        ))
    };
    let mut file = make()?;
    file.start_on(Some(Path::new("README.md")));
    assert!(file.menu_bar_shown());
    assert!(file.sidebar.tree && file.sidebar.threads);
    assert_eq!(file.focus(), Focus::View);

    let mut workspace = make()?;
    workspace.start_on(None);
    assert!(workspace.menu_bar_shown());
    assert!(workspace.sidebar.tree && workspace.sidebar.threads);
    assert_eq!(workspace.focus(), Focus::Tree);
    Ok(())
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
fn picker_scrolloff_allows_free_motion_between_margins() {
    let items: Vec<_> = (0..20).map(|index| format!("item {index}")).collect();
    let mut picker = PickerState::new(PickerKind::Files, items);
    let rows = 10;

    picker.move_by(7, rows);
    assert_eq!((picker.selected(), picker.first_visible(rows)), (7, 1));
    picker.move_by(-1, rows);
    assert_eq!((picker.selected(), picker.first_visible(rows)), (6, 1));
    picker.move_by(-1, rows);
    assert_eq!((picker.selected(), picker.first_visible(rows)), (5, 1));
    picker.move_by(-1, rows);
    assert_eq!((picker.selected(), picker.first_visible(rows)), (4, 1));
    picker.move_by(-1, rows);
    assert_eq!(
        (picker.selected(), picker.first_visible(rows)),
        (3, 0),
        "the list scrolls only after the cursor crosses the upper margin"
    );
    picker.move_by(1, rows);
    assert_eq!((picker.selected(), picker.first_visible(rows)), (4, 0));
    picker.move_by(1, rows);
    assert_eq!((picker.selected(), picker.first_visible(rows)), (5, 0));
    picker.move_by(1, rows);
    assert_eq!((picker.selected(), picker.first_visible(rows)), (6, 0));
    picker.move_by(1, rows);
    assert_eq!(
        (picker.selected(), picker.first_visible(rows)),
        (7, 1),
        "the cursor crosses the viewport freely before scrolling resumes"
    );

    picker.move_by(100, rows);
    assert_eq!(
        (picker.selected(), picker.first_visible(rows)),
        (19, 10),
        "the cursor approaches the edge only after the list reaches its end"
    );
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
    assert!(
        app.view()
            .layout()
            .lines()
            .iter()
            .flat_map(fathomable_core::layout::Line::spans)
            .all(|span| span.style().fg.is_none()),
        "the initial source layout does not wait for highlighting"
    );
    complete_highlights(&mut app);
    let coloured = app.view().layout().lines()[0]
        .spans()
        .iter()
        .any(|span| span.style().fg.is_some());
    assert!(coloured, "the source layout is highlighted by extension");
    assert!(
        !app.view_mut().toggle_source_view(),
        "a direct view call cannot render ordinary source"
    );
    press(&mut app, " vs");
    assert!(
        app.view().source_view(),
        "the key binding cannot bypass eligibility"
    );
    assert_eq!(
        app.message(),
        Some("rendered view is unavailable for this file")
    );
    app.command("source");
    assert!(
        app.view().source_view(),
        "the command cannot bypass eligibility"
    );
    assert!(
        app.view().layout().lines()[0]
            .spans()
            .iter()
            .any(|span| span.style().fg.is_some()),
        "failed render attempts retain source highlighting"
    );

    app.open(Path::new("README.md"));
    assert!(!app.view().source_view(), "Markdown opens rendered");
    assert_eq!(app.view().layout().lines()[0].text(), "Readme");
    app.open(Path::new("docs/guide.md"));
    app.command("source");
    assert!(app.view().source_view());
    app.open(Path::new("README.md"));
    assert!(
        !app.view().source_view(),
        "each eligible file retains its own display choice"
    );
    app.open(Path::new("docs/guide.md"));
    assert!(
        app.view().source_view(),
        "the source choice returns with its document"
    );
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

    Ok(())
}

#[test]
fn custom_markdown_eligibility_replaces_defaults_and_ignores_path_case() -> anyhow::Result<()> {
    let dir = fixture("custom-markdown")?;
    fs::write(dir.0.join("NOTES.RST"), "# Notes\n")?;
    fs::write(dir.0.join("GUIDE"), "# Guide\n")?;
    let mut app = AppBuilder::at(&dir.0)
        .unopened()
        .options(|o| Options {
            markdown: MarkdownConfig {
                extensions: vec!["rst".to_owned()],
                names: vec!["guide".to_owned()],
            },
            ..o
        })
        .build()?;
    app.open(Path::new("README.md"));
    assert!(
        app.view().source_view(),
        "custom configuration excludes default Markdown extensions"
    );
    app.open(Path::new("NOTES.RST"));
    assert!(
        !app.view().source_view(),
        "custom extensions match path case-insensitively"
    );
    app.open(Path::new("GUIDE"));
    assert!(
        !app.view().source_view(),
        "custom extensionless names match path case-insensitively"
    );
    Ok(())
}

#[test]
fn source_actions_require_an_open_document() -> anyhow::Result<()> {
    let dir = fixture("source-unopened")?;
    let mut app = AppBuilder::at(&dir.0).unopened().build()?;
    let welcome = app
        .view()
        .layout()
        .lines()
        .iter()
        .map(fathomable_core::layout::Line::text)
        .collect::<Vec<_>>();

    assert!(!app.source_view_available());
    press(&mut app, " vs");
    assert_eq!(app.message(), Some("no file open"));
    assert!(!app.view().source_view());
    app.command("source");
    assert_eq!(app.message(), Some("no file open"));
    assert!(!app.view().source_view());
    assert_eq!(
        app.view()
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .collect::<Vec<_>>(),
        welcome,
        "unavailable actions do not relayout the welcome placeholder"
    );
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
fn files_need_enter_to_take_focus_while_l_only_navigates_directories() -> anyhow::Result<()> {
    use crate::app::testing::{press, press_key};
    use crossterm::event::KeyCode;
    use fathomable_core::tree::Row;

    let dir = fixture("tree-enter-only")?;
    let mut app = app(&dir)?;
    app.show_tree();
    assert_eq!(
        app.tree().and_then(Tree::current).map(Row::path),
        Some(Path::new("docs"))
    );
    press(&mut app, "l");
    assert!(
        app.tree()
            .and_then(Tree::current)
            .is_some_and(Row::expanded)
    );
    press_key(&mut app, KeyCode::Right);
    assert_eq!(app.current_path(), Path::new("docs/guide.md"));
    assert_eq!(
        app.focus(),
        Focus::Tree,
        "descending previews without taking focus"
    );
    let cursor = app.view().cursor();
    for key in [KeyCode::Char('l'), KeyCode::Right] {
        press_key(&mut app, key);
        assert_eq!(app.focus(), Focus::Tree, "a file stays in the file list");
        assert_eq!(app.current_path(), Path::new("docs/guide.md"));
        assert_eq!(app.view().cursor(), cursor);
    }
    press(&mut app, "h");
    assert_eq!(
        app.tree().and_then(Tree::current).map(Row::path),
        Some(Path::new("docs"))
    );
    press(&mut app, "h");
    assert!(
        app.tree()
            .and_then(Tree::current)
            .is_some_and(|row| !row.expanded())
    );
    press(&mut app, "ll");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.focus(), Focus::View, "Enter commits focus to the text");
    assert_eq!(app.current_path(), Path::new("docs/guide.md"));
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
            0 => {
                app.view_mut().toggle_source_view();
            }
            1 => {
                app.view_mut().set_bases(None, Some(String::new()));
                app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
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
        "the loaded document stays available behind the directory card"
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
fn a_directory_highlight_shows_its_summary_instead_of_the_last_file() -> anyhow::Result<()> {
    use crossterm::event::KeyCode;
    use fathomable_core::annotations::{Author, Draft, LineRange, Reply, Store};

    use crate::app::testing::{press, press_key, screen};

    let dir = fixture("directory-summary")?;
    fs::create_dir_all(dir.0.join("docs/reference"))?;
    fs::write(dir.0.join("docs/reference/api.md"), "# API\n")?;
    git::init(&dir.0)?;
    git::commit_and_stage(
        &dir.0,
        &[
            ("README.md", "# Readme\n\nhello\n"),
            ("docs/guide.md", "# Guide\n"),
            ("docs/notes.md", "# Notes\n"),
            ("docs/reference/api.md", "# API\n"),
        ],
    )?;
    fs::write(dir.0.join("docs/guide.md"), "# Guide\n\nchanged\n")?;
    fs::write(dir.0.join("docs/draft.md"), "draft\n")?;

    let store_path = dir.0.join(".state/threads.jsonl");
    let mut store = Store::open(&store_path)?;
    store.annotate(
        Draft::new(
            Author::User,
            Path::new("docs/guide.md"),
            LineRange::new(1, 1),
            "open",
        ),
        "# Guide\n\nchanged\n",
        1,
    )?;
    let answered = store.annotate(
        Draft::new(
            Author::User,
            Path::new("docs/notes.md"),
            LineRange::new(1, 1),
            "question",
        ),
        "# Notes\n",
        2,
    )?;
    store.reply(&answered, Reply::new(Author::agent("agent"), 3, "answer"))?;

    let mut app = app_with(
        &dir,
        Options {
            store: Some(Store::open(&store_path)?),
            ..Options::for_test(dir.0.clone())
        },
    )?;
    app.settle_status();
    app.open(Path::new("README.md"));
    app.toggle_tree_focus();
    press(&mut app, "k");

    let info = app
        .directory_info()
        .ok_or_else(|| anyhow::anyhow!("directory summary not shown"))?;
    assert_eq!(info.path, Path::new("docs"));
    assert_eq!((info.files, info.subdirectories), (Some(3), Some(1)));
    assert_eq!(
        (
            info.changed_files,
            info.added,
            info.removed,
            info.active_threads,
            info.proposed_threads,
            info.resolved_threads,
        ),
        (2, 3, 0, 2, 0, 0)
    );
    let shown = screen(&app)?;
    let chrome = &shown[app.pane_top()];
    assert!(chrome.contains("File  docs/"), "{chrome}");
    let output = shown.join("\n");
    assert!(output.contains("docs/"), "{output}");
    assert!(output.contains("files  3"), "{output}");
    assert!(output.contains("subdirectories  1"), "{output}");
    assert!(output.contains("changes  2 files · +3 -0"), "{output}");
    assert!(output.contains("threads  ● 2 active"), "{output}");
    assert!(!output.contains("Readme"), "{output}");

    press_key(&mut app, KeyCode::Esc);
    assert!(app.directory_info().is_none());
    assert!(screen(&app)?.join("\n").contains("Readme"));
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

/// The picker's index follows the events rather than being walked
/// again after each (ADR 0028): a file or a whole directory that
/// appears is listed where the walk would put it, a removed or renamed
/// path leaves, and an ignore rules change walks again.
#[test]
fn the_picker_index_follows_events() -> anyhow::Result<()> {
    use super::watch::Event;

    let dir = fixture("picker-index")?;
    git::init(&dir.0)?;
    let mut app = app(&dir)?;
    let listed = |app: &mut App, kind: PickerKind| -> Vec<String> {
        app.open_picker(kind);
        let items = picker_items(app);
        app.close_popup();
        items
    };
    assert_eq!(
        listed(&mut app, PickerKind::Files),
        ["README.md", "docs/guide.md", "docs/notes.md"]
    );

    // A new file, and a directory that arrived whole with one event.
    fs::write(dir.0.join("Cargo.toml"), "[package]\n")?;
    fs::create_dir_all(dir.0.join("crates/pipe/src"))?;
    fs::write(dir.0.join("crates/pipe/src/lib.rs"), "")?;
    fs::write(dir.0.join("crates/pipe/Cargo.toml"), "")?;
    app.on_events(vec![
        Event::Created(dir.0.join("Cargo.toml")),
        Event::Created(dir.0.join("crates")),
    ]);
    assert_eq!(
        listed(&mut app, PickerKind::Files),
        [
            "Cargo.toml",
            "README.md",
            "crates/pipe/Cargo.toml",
            "crates/pipe/src/lib.rs",
            "docs/guide.md",
            "docs/notes.md",
        ]
    );

    // A rename moves the path; a removal takes a directory's files along.
    fs::rename(dir.0.join("docs/notes.md"), dir.0.join("NOTES.md"))?;
    fs::remove_dir_all(dir.0.join("crates"))?;
    app.on_events(vec![
        Event::Renamed {
            from: dir.0.join("docs/notes.md"),
            to: dir.0.join("NOTES.md"),
        },
        Event::Removed(dir.0.join("crates")),
    ]);
    assert_eq!(
        listed(&mut app, PickerKind::Files),
        ["Cargo.toml", "NOTES.md", "README.md", "docs/guide.md"]
    );

    // An ignored file is listed only with `I`; a rules change walks again.
    fs::write(dir.0.join("out.log"), "")?;
    fs::write(dir.0.join(".gitignore"), "*.log\n")?;
    app.on_events(vec![
        Event::Created(dir.0.join(".gitignore")),
        Event::Created(dir.0.join("out.log")),
    ]);
    assert!(!listed(&mut app, PickerKind::Files).contains(&"out.log".to_owned()));
    assert!(listed(&mut app, PickerKind::AllFiles).contains(&"out.log".to_owned()));
    Ok(())
}

#[test]
fn unchanged_loaded_content_emits_no_file_edit_toast() -> anyhow::Result<()> {
    let dir = fixture("touch")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    app.open(Path::new("docs/guide.md"));
    // A touch (or our own read, reported as a change) on an open file
    // whose text is identical must not hint.
    app.on_changes(vec![dir.0.join("README.md"), dir.0.join("README.md")]);
    assert!(app.toasts().is_empty());
    Ok(())
}

/// Board membership is ancestry-independent; every thread is projected into
/// the displayed content or represented as detached. Another writer's append
/// reaches the viewer through the store watch.
#[test]
fn threads_follow_the_work_and_other_writers_are_picked_up() -> anyhow::Result<()> {
    use fathomable_core::annotations::{Author, Draft, LineRange, Store};

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
    let other = store.annotate(
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
    assert_eq!(ids, [here.clone(), other.clone(), unscoped.clone()]);

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
    assert_eq!(ids, [here, other, unscoped, late]);
    Ok(())
}

/// An amend replaces `HEAD` with a commit that does not descend from
/// it. Origins remain pinned, while the shared board still lists the
/// open discussions even when this checkout cannot project them.
#[test]
fn open_threads_keep_origin_across_an_amend() -> anyhow::Result<()> {
    use fathomable_core::annotations::{Author, Draft, LineRange, Store, Thread};

    let dir = fixture("origin-amend")?;
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
    store.resolve(&done, Some(&first), 4)?;

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
    assert_eq!(ids, [kept.clone(), gone.clone(), done.clone()]);
    assert!(
        app.marks()
            .iter()
            .find(|mark| mark.id() == &gone)
            .is_some_and(crate::app::threads::Mark::is_detached)
    );
    let again = Store::open(&store_path)?;
    assert_eq!(
        again.thread(&kept).and_then(Thread::commit),
        Some(first.as_str())
    );
    assert_eq!(
        again.thread(&gone).and_then(Thread::commit),
        Some(first.as_str())
    );
    assert_eq!(
        again.thread(&done).and_then(Thread::commit),
        Some(first.as_str())
    );
    assert_eq!(app.review_entries(false).len(), 2);
    Ok(())
}

/// Resolution records its checkout commit without making later `HEAD`
/// equality a board or placement-membership rule.
#[test]
fn a_resolved_thread_remains_projectable_after_the_next_commit() -> anyhow::Result<()> {
    use fathomable_core::annotations::{Author, Draft, LineRange, Store, Thread};

    let dir = fixture("past")?;
    git::init(&dir.0)?;
    let text = "# Readme\n\nhello\n";
    git::commit_and_stage(&dir.0, &[("README.md", text)])?;
    let workspace = Workspace::discover(&dir.0)?;
    let first = workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
    let store_path = dir.0.join(".state/threads.jsonl");
    let mut store = Store::open(&store_path)?;
    let old = store.annotate(
        Draft::new(
            Author::User,
            Path::new("README.md"),
            LineRange::new(3, 3),
            "older",
        )
        .at_commit(Some(first.clone())),
        text,
        1,
    )?;
    // A second commit: the thread's commit is now an ancestor.
    git::commit_and_stage(
        &dir.0,
        &[("README.md", text), ("docs/guide.md", "# Guide\n\nmore\n")],
    )?;
    let second = Workspace::discover(&dir.0)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
    assert_ne!(first, second);

    let mut app = app_with(
        &dir,
        Options {
            store: Some(Store::open(&store_path)?),
            ..Options::for_test(dir.0.clone())
        },
    )?;
    app.open(Path::new("README.md"));
    assert_eq!(app.marks().len(), 1);

    // `o` at the second commit: the thread moves there and still shows.
    app.set_thread_cursor(old.clone());
    app.thread_toggle_resolved();
    assert_eq!(app.message(), Some("resolved"));
    assert_eq!(app.marks().len(), 1, "resolved at HEAD still shows");
    assert_eq!(
        Store::open(&store_path)?
            .thread(&old)
            .and_then(Thread::commit),
        Some(second.as_str())
    );

    // A third commit: the resolved thread remains projectable and stays in
    // repository history, hidden only by the normal resolved filter.
    git::commit_and_stage(
        &dir.0,
        &[("README.md", text), ("docs/notes.md", "# Notes\n\nmore\n")],
    )?;
    app.on_changes(vec![dir.0.join(".git/HEAD")]);
    assert_eq!(app.marks().len(), 1, "still projected in displayed content");
    assert!(app.review_entries(false).is_empty(), "hidden until x");
    app.review_toggle_resolved();
    let entries = app.review_entries(false);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].commit(), Some(&second[..7]));
    assert_eq!(entries[0].range(), Some(LineRange::new(3, 3)));
    assert_eq!(app.review_counts(false).resolved, 1);

    // In the list the complete conversation remains usable. A reply draft
    // preserves the normal resolved-thread confirmation, and Enter can open
    // the trustworthy placement.
    app.open_review();
    app.set_thread_cursor(old.clone());
    app.thread_reply();
    assert!(app.draft().is_some());
    app.compose_cancel();
    app.thread_open_in_file();
    assert!(!app.review_list().is_open());
    assert_eq!(app.view().cursor_source_line(), Some(3));
    app.open_review();
    app.set_thread_cursor(old.clone());
    app.thread_toggle_resolved();
    assert_eq!(app.message(), Some("reopened"));
    assert_eq!(app.marks().len(), 1, "open again, on the work");
    assert_eq!(app.review_entries(false)[0].commit(), None);
    Ok(())
}

fn changed(app: &mut App, dir: &TempDir, relative: &str, text: &str) -> std::io::Result<()> {
    let absolute = dir.0.join(relative);
    fs::write(&absolute, text)?;
    app.on_changes(vec![absolute]);
    Ok(())
}

#[test]
fn shared_toast_cap_and_expiry_apply_to_file_and_plain_toasts() -> anyhow::Result<()> {
    let dir = fixture("toast-cap-expiry")?;
    let mut app = app(&dir)?;
    app.push_toast("plain one".to_owned());
    app.push_file_edit_toast(PathBuf::from("one.md"), (1, 0));
    app.push_toast("plain two".to_owned());
    app.push_file_edit_toast(PathBuf::from("two.md"), (0, 1));
    assert_eq!(app.toasts().len(), super::MAX_TOASTS);
    assert_eq!(
        app.toasts().first().map(super::Toast::text),
        Some("one.md  +1")
    );

    app.toasts[0].until = std::time::Instant::now();
    app.tick();
    assert_eq!(app.toasts().len(), 2);
    assert_eq!(
        app.toasts().first().map(super::Toast::text),
        Some("plain two")
    );
    Ok(())
}

#[test]
fn off_hides_file_edit_toasts_without_discarding_them() -> anyhow::Result<()> {
    let dir = fixture("live-toast-off")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    changed(
        &mut app,
        &dir,
        "docs/live-toast-sentinel.md",
        "# live toast sentinel\n",
    )?;
    assert_eq!(app.toasts().len(), 1);
    let active = crate::app::testing::screen(&app)?.join("\n");
    assert!(active.contains("live-toast-sentinel"), "{active}");

    app.select_diff_mode(DiffMode::Off);
    assert_eq!(app.toasts().len(), 1, "Off retains timed toasts");
    let off = crate::app::testing::screen(&app)?.join("\n");
    assert!(!off.contains("live-toast-sentinel"), "{off}");
    assert_eq!(app.current_path(), Path::new("README.md"));

    app.select_diff_mode(DiffMode::Standard);
    let restored = crate::app::testing::screen(&app)?.join("\n");
    assert!(restored.contains("live-toast-sentinel"), "{restored}");

    app.select_diff_mode(DiffMode::Off);
    app.toasts[0].until = std::time::Instant::now();
    app.tick();
    assert!(app.toasts().is_empty(), "hidden toast expires normally");
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

    // Committing everything empties current Git status, but the pinned
    // comparison intentionally remains anchored to its original HEAD.
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
    assert_eq!(app.view().diff_counts(), Some((4, 0)));
    Ok(())
}

#[test]
fn loaded_edit_toast_counts_each_reload_without_navigating() -> anyhow::Result<()> {
    let dir = fixture("loaded-change-toast")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    changed(&mut app, &dir, "README.md", "# Readme\n\nchanged\nextra\n")?;
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.view().text(), "# Readme\n\nchanged\nextra\n");
    assert_eq!(
        app.toasts().last().map(super::Toast::text),
        Some("README.md  +2  -1")
    );

    changed(&mut app, &dir, "README.md", "# Readme\n\nagain\n")?;
    assert_eq!(
        app.toasts().last().map(super::Toast::text),
        Some("README.md  +1  -2")
    );
    Ok(())
}

#[test]
fn unloaded_git_toasts_use_bounded_head_relative_counts() -> anyhow::Result<()> {
    let dir = fixture("unloaded-counts")?;
    git::init(&dir.0)?;
    git::commit_and_stage(
        &dir.0,
        &[
            ("README.md", "# Readme\n\nhello\n"),
            ("docs/guide.md", "# Guide\nold\n"),
        ],
    )?;
    let mut app = app(&dir)?;
    changed(&mut app, &dir, "docs/guide.md", "# Guide\nnew\nextra\n")?;
    assert_eq!(
        app.toasts().last().map(super::Toast::text),
        Some("docs/guide.md  +2  -1")
    );

    changed(&mut app, &dir, "new.md", "one\ntwo\n")?;
    assert_eq!(
        app.toasts().last().map(super::Toast::text),
        Some("new.md  +2")
    );
    Ok(())
}

#[test]
fn unborn_git_counts_every_unloaded_line_as_added() -> anyhow::Result<()> {
    let dir = fixture("unborn-counts")?;
    git::init(&dir.0)?;
    let mut app = app(&dir)?;
    changed(&mut app, &dir, "new.md", "one\ntwo\nthree\n")?;
    assert_eq!(
        app.toasts().last().map(super::Toast::text),
        Some("new.md  +3")
    );
    Ok(())
}

#[test]
fn unavailable_unloaded_content_falls_back_to_path_only() -> anyhow::Result<()> {
    let outside = fixture("outside-git-toast")?;
    let mut outside_app = app(&outside)?;
    changed(&mut outside_app, &outside, "plain.md", "readable\n")?;
    assert_eq!(
        outside_app.toasts().last().map(super::Toast::text),
        Some("plain.md")
    );

    let binary = fixture("binary-toast")?;
    git::init(&binary.0)?;
    git::commit_and_stage(&binary.0, &[("README.md", "# Readme\n\nhello\n")])?;
    let mut binary_app = app(&binary)?;
    fs::write(binary.0.join("blob.bin"), b"abc\0def")?;
    binary_app.on_changes(vec![binary.0.join("blob.bin")]);
    assert_eq!(
        binary_app.toasts().last().map(super::Toast::text),
        Some("blob.bin")
    );
    fs::write(binary.0.join("invalid.txt"), b"\xff\xfe")?;
    binary_app.on_changes(vec![binary.0.join("invalid.txt")]);
    assert_eq!(
        binary_app.toasts().last().map(super::Toast::text),
        Some("invalid.txt")
    );

    let unavailable = fixture("unavailable-head-toast")?;
    git::init(&unavailable.0)?;
    git::commit_and_stage(
        &unavailable.0,
        &[
            ("README.md", "# Readme\n\nhello\n"),
            ("node/child", "old\n"),
        ],
    )?;
    let mut unavailable_app = app(&unavailable)?;
    changed(&mut unavailable_app, &unavailable, "node", "now a file\n")?;
    assert_eq!(
        unavailable_app.toasts().last().map(super::Toast::text),
        Some("node")
    );

    let over_limit = fixture("over-limit-toast")?;
    git::init(&over_limit.0)?;
    git::commit_and_stage(
        &over_limit.0,
        &[("README.md", "# Readme\n\nhello\n"), ("large.md", "old\n")],
    )?;
    let mut over_limit_app = app_with(
        &over_limit,
        Options {
            viewer: ViewerConfig {
                max_file_size_mib: 0,
            },
            ..Options::for_test(over_limit.0.clone())
        },
    )?;
    changed(&mut over_limit_app, &over_limit, "large.md", "new\n")?;
    assert_eq!(
        over_limit_app.toasts().last().map(super::Toast::text),
        Some("large.md")
    );

    let large_head = fixture("over-limit-head-toast")?;
    git::init(&large_head.0)?;
    let committed = "old\n".repeat(300_000);
    git::commit_and_stage(
        &large_head.0,
        &[
            ("README.md", "# Readme\n\nhello\n"),
            ("large.md", &committed),
        ],
    )?;
    let mut large_head_app = app_with(
        &large_head,
        Options {
            viewer: ViewerConfig {
                max_file_size_mib: 1,
            },
            ..Options::for_test(large_head.0.clone())
        },
    )?;
    changed(&mut large_head_app, &large_head, "large.md", "small\n")?;
    assert_eq!(
        large_head_app.toasts().last().map(super::Toast::text),
        Some("large.md")
    );
    Ok(())
}

#[test]
fn loaded_text_to_non_text_transition_keeps_empty_text_counts() -> anyhow::Result<()> {
    let dir = fixture("loaded-binary-transition")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    fs::write(dir.0.join("README.md"), b"\0asm\x01\0\0\0")?;
    app.on_changes(vec![dir.0.join("README.md")]);
    assert_eq!(
        app.toasts().last().map(super::Toast::text),
        Some("README.md  -3")
    );
    fs::write(dir.0.join("README.md"), "back\n")?;
    app.on_changes(vec![dir.0.join("README.md")]);
    assert_eq!(
        app.toasts().last().map(super::Toast::text),
        Some("README.md  +1")
    );
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
    app.settle_status();
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

    // A path `watch.ignore` hides never triggers a re-read.
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
fn ignored_churn_is_dropped_except_for_the_open_file() -> anyhow::Result<()> {
    use super::watch::{Event, Raw};

    let dir = fixture("ignored-events")?;
    git::init(&dir.0)?;
    fs::write(dir.0.join(".gitignore"), "target/\n")?;
    fs::create_dir_all(dir.0.join("target/deep"))?;
    let open = dir.0.join("target/deep/open.txt");
    let sibling = dir.0.join("target/deep/noise.bin");
    fs::write(&open, "one\n")?;
    fs::write(&sibling, "noise\n")?;
    let mut app = app(&dir)?;
    app.open(Path::new("target/deep/open.txt"));

    assert!(app.raw_is_relevant(&Raw::Modify(open.clone())));
    assert!(!app.raw_is_relevant(&Raw::Modify(sibling)));
    fs::write(&open, "two\n")?;
    app.on_events(vec![Event::Change(open)]);
    assert_eq!(app.view().text(), "two\n");
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
    app.settle_status();
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
fn a_write_during_the_full_walk_lands_in_the_set() -> anyhow::Result<()> {
    use fathomable_core::status::State;

    use super::watch::Event;

    let dir = fixture("status-walk")?;
    git::init(&dir.0)?;
    git::commit_and_stage(&dir.0, &[("README.md", "# Readme\n\nhello\n")])?;
    let mut app = app(&dir)?;
    let dirty = |app: &App| -> Vec<(String, State)> {
        app.status()
            .entries()
            .iter()
            .map(|e| (e.path().display().to_string(), e.state()))
            .collect()
    };
    let untracked = |name: &str| (name.to_owned(), State::Untracked);
    assert_eq!(
        dirty(&app),
        vec![untracked("docs/guide.md"), untracked("docs/notes.md")]
    );

    // Lost events start a walk on its own thread; the set in hand stays
    // until it lands, and a file written meanwhile is examined again on
    // the result, whether or not the walk saw it.
    app.on_events(vec![Event::Rescan]);
    fs::write(dir.0.join("NEW.md"), "# New\n")?;
    app.on_events(vec![Event::Created(dir.0.join("NEW.md"))]);
    assert!(app.status().contains(Path::new("NEW.md")), "seen at once");
    // A second walk supersedes the first; the earlier result is dropped.
    app.on_events(vec![Event::Rescan]);
    fs::write(dir.0.join("docs/guide.md"), "# Guide\n\nmore\n")?;
    app.on_events(vec![Event::Change(dir.0.join("docs/guide.md"))]);
    app.settle_status();
    assert_eq!(
        dirty(&app),
        vec![
            untracked("NEW.md"),
            untracked("docs/guide.md"),
            untracked("docs/notes.md"),
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
fn ignore_rules_filter_toasts_but_not_reloads() -> anyhow::Result<()> {
    let dir = fixture("source")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    changed(&mut app, &dir, "README.md", "# Readme\n\nchanged\n")?;
    assert_eq!(app.toasts().len(), 1, "an edit to the open file toasts");
    assert!(
        app.view().text().contains("changed"),
        "and the open file reloads"
    );

    let watch = WatchConfig {
        toast: std::time::Duration::ZERO,
        ignore: vec!["docs/**".to_owned()],
        ..WatchConfig::default()
    };
    let mut app = app_with(
        &dir,
        Options {
            watch,
            ..Options::for_test(dir.0.clone())
        },
    )?;
    changed(&mut app, &dir, "docs/guide.md", "# Guide\n\n3\n")?;
    assert!(app.toasts().is_empty(), "watch.ignore globs apply");
    changed(&mut app, &dir, "README.md", "# Readme\n\nx\n")?;
    assert!(app.toasts().is_empty(), "toast 0 disables toasts");

    app.command("status");
    assert!(matches!(app.popup(), Some(Popup::Status)));
    app.close_popup();
    app.command("help");
    assert!(app.getting_started());
    app.command("help");
    assert!(!app.getting_started());
    app.command("about");
    assert!(matches!(app.popup(), Some(Popup::About)));
    app.close_popup();
    app.command("nonsense");
    assert!(
        app.message()
            .is_some_and(|m| m.starts_with("not a command"))
    );
    Ok(())
}

#[test]
fn unavailable_thread_store_points_to_doctor() -> anyhow::Result<()> {
    let dir = fixture("threads-unavailable")?;
    let thread_file = dir.0.join("threads.jsonl");
    fathomable_core::private_state::write(&thread_file, "{\"v\":3}\n")?;
    let Err(error) = Store::open(thread_file) else {
        return Err(anyhow::anyhow!("stale thread store unexpectedly opened"));
    };
    let mut app = app_with(
        &dir,
        Options {
            thread_store_error: Some(error),
            ..Options::for_test(dir.0.clone())
        },
    )?;
    app.open(Path::new("README.md"));

    app.start_new_comment();

    assert_eq!(
        app.message(),
        Some(
            "threads unavailable: incompatible storage versions (3 on disk, 5 expected); run :doctor"
        )
    );
    assert_eq!(app.message_tone(), NoticeTone::Error);
    let rows = app.status_lines();
    let log = rows
        .iter()
        .find_map(|(label, value)| (label == "log").then_some(value));
    assert_eq!(
        log.map(String::as_str),
        Some(
            crate::logging::log_path(&app.dirs, app.record.id())
                .to_string_lossy()
                .as_ref()
        )
    );
    assert!(
        rows.iter()
            .any(|(label, value)| label == "threads" && value == "unavailable; run :doctor")
    );
    Ok(())
}

#[test]
fn startup_shared_state_warning_survives_opening_the_requested_file() -> anyhow::Result<()> {
    let dir = fixture("shared-state-startup")?;
    let mut app = AppBuilder::at(&dir.0)
        .options(|mut options| {
            options.shared_state_ancestor_count = 2;
            options
        })
        .build()?;
    assert_eq!(
        app.message(),
        Some("2 group-writable state ancestors; run :doctor for details")
    );
    assert_eq!(app.message_tone(), NoticeTone::Warning);
    assert!(app.tick_in().is_some());
    if let Some(notice) = app.message.as_mut() {
        notice.until = Some(std::time::Instant::now());
    }
    app.tick();
    assert_eq!(app.message(), None);

    let clean = AppBuilder::at(&dir.0).build()?;
    assert_eq!(clean.message(), None);

    let mut failed = AppBuilder::at(&dir.0)
        .options(|mut options| {
            options.shared_state_ancestor_count = 1;
            options
        })
        .build()?;
    if let Some(notice) = failed.message.as_mut() {
        notice.until = Some(std::time::Instant::now());
    }
    failed.error("cannot open requested file");
    assert_eq!(failed.tick_in(), None);
    failed.tick();
    assert_eq!(failed.message_tone(), NoticeTone::Error);
    assert_eq!(failed.message(), Some("cannot open requested file"));
    Ok(())
}

#[test]
fn file_edit_toasts_never_move_the_reader() -> anyhow::Result<()> {
    let dir = fixture("change-toast-no-navigation")?;
    let mut app = app(&dir)?;
    app.open(Path::new("README.md"));
    changed(&mut app, &dir, "docs/notes.md", "# Notes\n\nnew\n")?;
    assert!(app.tick_in().is_some());
    app.tick();
    assert_eq!(
        app.current_path(),
        Path::new("README.md"),
        "background changes never move the reader"
    );
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.toasts().len(), 1);
    Ok(())
}

#[test]
fn file_edits_do_not_mark_files_or_collapsed_directories() -> anyhow::Result<()> {
    let dir = fixture("change-toast-no-tree-dot")?;
    let mut app = app(&dir)?;
    app.toggle_tree_focus();
    changed(&mut app, &dir, "docs/notes.md", "# Notes\nnew\n")?;
    let screen = crate::app::testing::screen(&app)?;
    let docs = screen
        .iter()
        .find(|row| row.contains("docs/"))
        .ok_or_else(|| anyhow::anyhow!("docs tree row"))?;
    assert!(!docs.contains('●'), "{docs}");
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
    let origin = app
        .jump_origin()
        .ok_or_else(|| anyhow::anyhow!("current jumplist position"))?;
    app.record_jump(origin);
    app.open(Path::new("README.md"));
    assert!(app.info().is_none());
    app.jump_back();
    assert_eq!(app.current_path(), Path::new("big.log"));
    assert!(app.info().is_some());
    Ok(())
}
