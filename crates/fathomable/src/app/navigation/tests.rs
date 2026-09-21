use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use fathomable_core::annotations::{Author, Draft, LineRange, OriginSide, Reply, Store};
use fathomable_core::config::DiffMode;
use fathomable_core::highlight::Highlighter;
use fathomable_core::tree::{Rule, Tree};
use fathomable_testing::{TempDir, git};

use super::stepped_hunk_index;
use crate::app::input::bindings::Action;
use crate::app::input::keys::handle_key;
use crate::app::testing::AppBuilder;
use crate::app::{App, Focus, testing};

fn deletion_and_later_change(name: &str, mode: DiffMode) -> anyhow::Result<(TempDir, App)> {
    let base = (1..=100)
        .map(|line| {
            if line == 20 {
                format!("line {line} {}", "wrapped deletion ".repeat(8))
            } else {
                format!("line {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let target = (1..=100)
        .filter(|line| !(20..=22).contains(line))
        .map(|line| {
            if line == 75 {
                "changed 75".to_owned()
            } else if line == 20 {
                unreachable!("deleted lines were filtered")
            } else {
                format!("line {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let dir = testing::workspace(name, &base)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", &base)])?;
    fs::write(root.join("README.md"), target)?;
    let mut app = AppBuilder::new(&dir).source_view().build()?;
    app.resize(36, 12);
    app.select_diff_mode(mode);
    app.settle_background();
    assert_eq!(app.view().hunks().len(), 2);
    Ok((dir, app))
}

#[test]
fn exact_hunk_index_keeps_equal_target_lines_distinct() {
    let lines = [4, 4, 9];
    assert_eq!(stepped_hunk_index(&lines, 4, None, true), Some(2));
    assert_eq!(stepped_hunk_index(&lines, 4, Some(0), true), Some(1));
    assert_eq!(stepped_hunk_index(&lines, 4, Some(1), false), Some(0));
}

#[test]
fn metadata_only_path_is_one_comparison_stop() -> anyhow::Result<()> {
    let dir = TempDir::new("navigation-mode-only")?;
    let root = &dir.0;
    fs::write(root.join("README.md"), "one\ntwo\nthree\n")?;
    git::init(root)?;
    git::commit_and_stage(root, &[("README.md", "one\ntwo\nthree\n")])?;
    let mut permissions = fs::metadata(root.join("README.md"))?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(root.join("README.md"), permissions)?;

    let mut app = AppBuilder::at(root).source_view().build()?;
    assert_eq!(app.comparison_status().entries().len(), 1);
    assert!(app.view().hunk_target_lines().is_empty());
    app.view_mut().goto_source_line(3);
    app.hunk_next();
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.view().cursor_source_line(), Some(1));
    assert_eq!(app.message(), Some("wrapped to first change"));
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), None);
    Ok(())
}

#[test]
fn same_file_hunk_landing_opens_and_focuses_file() -> anyhow::Result<()> {
    let base = (1..=20)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let dir = testing::workspace("navigation-same-file", &base)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", &base)])?;
    let changed = base
        .replace("line 5", "changed 5")
        .replace("line 15", "changed 15");
    fs::write(root.join("README.md"), changed)?;

    let mut app = AppBuilder::new(&dir).source_view().build()?;
    app.view_mut().goto_source_line(1);
    app.open_review();
    assert!(app.review_list().is_open());

    app.hunk_next();
    assert_eq!(app.focus(), Focus::View);
    assert!(!app.review_list().is_open());
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.view().cursor_source_line(), Some(5));

    handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    assert_eq!(app.view().cursor_source_line(), Some(15));
    testing::press(&mut app, "K");
    assert_eq!(app.view().cursor_source_line(), Some(5));
    Ok(())
}

#[test]
fn changed_file_navigation_always_lands_on_the_first_hunk() -> anyhow::Result<()> {
    let base = (1..=20)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let dir = testing::workspace("navigation-changed-files", &base)?;
    let root = testing::root(&dir);
    fs::write(root.join("z.txt"), &base)?;
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", &base), ("z.txt", &base)])?;
    let changed = base
        .replace("line 5", "changed 5")
        .replace("line 15", "changed 15");
    fs::write(root.join("README.md"), &changed)?;
    fs::write(root.join("z.txt"), &changed)?;

    let mut app = AppBuilder::new(&dir).source_view().build()?;
    assert_eq!(app.view().hunk_target_lines(), vec![5, 15]);
    app.view_mut().goto_source_line(15);

    app.changed_file_next();
    assert_eq!(app.current_path(), Path::new("z.txt"));
    assert_eq!(app.view().cursor_source_line(), Some(5));

    app.changed_file_prev();
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.view().cursor_source_line(), Some(5));

    testing::press(&mut app, "L");
    assert_eq!(app.current_path(), Path::new("z.txt"));
    assert_eq!(app.view().cursor_source_line(), Some(5));

    app.act(Action::ChangeFilePrev);
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.view().cursor_source_line(), Some(5));

    testing::press(&mut app, "H");
    assert_eq!(app.current_path(), Path::new("z.txt"));
    assert_eq!(app.view().cursor_source_line(), Some(5));

    handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT));
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.view().cursor_source_line(), Some(5));
    Ok(())
}

#[test]
fn unified_pure_deletion_lands_first_removed_row_at_top_third() -> anyhow::Result<()> {
    let base = (1..=80)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let target = (1..=40)
        .map(|line| {
            if line == 10 {
                "changed 10".to_owned()
            } else {
                format!("line {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let dir = testing::workspace("navigation-unified-deletion", &base)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", &base)])?;
    fs::write(root.join("README.md"), target)?;

    let mut app = AppBuilder::new(&dir).build()?;
    app.select_diff_mode(DiffMode::Unified);
    app.settle_background();
    app.hunk_next();
    app.hunk_next();

    let hunk = app
        .view()
        .hunks()
        .into_iter()
        .next_back()
        .ok_or_else(|| anyhow::anyhow!("hunk"))?;
    let rows = app
        .view()
        .hunk_rows(&hunk, false)
        .ok_or_else(|| anyhow::anyhow!("rendered rows"))?;
    let anchor = (app.view().body_height() - 1) / 3;
    assert_eq!(app.view().scroll() + anchor, rows.start);
    assert!(
        app.view().layout().lines()[rows.start]
            .spans()
            .iter()
            .any(|span| span.style().face == fathomable_core::layout::Face::DiffRemoved)
    );

    app.resize(72, 18);
    let hunk = app
        .view()
        .hunks()
        .into_iter()
        .next_back()
        .ok_or_else(|| anyhow::anyhow!("resized hunk"))?;
    let rows = app
        .view()
        .hunk_rows(&hunk, false)
        .ok_or_else(|| anyhow::anyhow!("resized rows"))?;
    let anchor = (app.view().body_height() - 1) / 3;
    assert_eq!(
        app.view().scroll() + anchor,
        rows.start,
        "resize re-resolves the logical hunk"
    );
    assert_eq!(
        app.change_stop
            .as_ref()
            .map(|stop| (stop.old.clone(), stop.new.clone())),
        Some((hunk.old_range(), hunk.new_range())),
        "passive placement retains the logical hunk identity"
    );
    Ok(())
}

#[test]
fn rendered_markdown_shared_row_traverses_exact_hunks_both_ways() -> anyhow::Result<()> {
    let prefix = (1..=10)
        .map(|line| format!("# Before {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let suffix = (1..=20)
        .map(|line| format!("# After {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let base = format!("{prefix}\n\none\ntwo\na\nb\nc\nd\ne\nf\ng\nh\nnine\nten\n\n{suffix}\n");
    let target = format!("{prefix}\n\none\nTWO\na\nb\nc\nd\ne\nf\ng\nh\nNINE\nten\n\n{suffix}\n");
    let dir = testing::workspace("navigation-rendered-shared-row", &base)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", &base)])?;
    fs::write(root.join("README.md"), target)?;
    let mut app = AppBuilder::new(&dir).build()?;
    app.settle_background();

    let mut shared = None;
    for width in 36..=64 {
        app.resize(width, 18);
        let hunks = app.view().hunks();
        if hunks.len() == 2
            && let (Some(first), Some(second)) = (
                app.view().hunk_rows(&hunks[0], false),
                app.view().hunk_rows(&hunks[1], false),
            )
            && first == second
        {
            shared = Some((width, first));
            break;
        }
    }
    let (width, shared) = shared.context("narrow layout sharing both hunk geometries")?;
    assert!(width <= 64, "fixture must remain a narrow rendered view");
    let hunks = app.view().hunks();
    let covered = app
        .view()
        .source_lines_of_row(shared.start)
        .context("shared row source coverage")?;
    assert!(covered.start() < hunks[0].new_range().start + 1);
    assert!(covered.contains(hunks[0].new_range().start + 1));
    assert!(covered.contains(hunks[1].new_range().start + 1));
    app.view_mut().goto_top();

    testing::press(&mut app, "J");
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), Some(0));
    assert_eq!(app.view().cursor().row, shared.start);
    assert_eq!(
        app.view().scroll() + (app.view().body_height() - 1) / 3,
        shared.start
    );

    testing::press(&mut app, "J");
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), Some(1));
    assert_eq!(app.view().cursor().row, shared.start);
    assert_eq!(
        app.view().scroll() + (app.view().body_height() - 1) / 3,
        shared.start
    );

    testing::press(&mut app, "J");
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), Some(0));
    assert_eq!(app.message(), Some("wrapped to first change"));
    testing::press(&mut app, "K");
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), Some(1));
    assert_eq!(app.message(), Some("wrapped to last change"));
    assert_eq!(
        app.view().scroll() + (app.view().body_height() - 1) / 3,
        shared.start
    );
    Ok(())
}

#[test]
fn normal_entire_deletion_lands_on_first_old_side_line() -> anyhow::Result<()> {
    let base = (1..=40)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let dir = testing::workspace("navigation-entire-deletion", &base)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", &base)])?;
    fs::remove_file(root.join("README.md"))?;

    let mut app = AppBuilder::new(&dir).build()?;
    app.hunk_next();

    assert_eq!(app.focus(), Focus::View);
    assert_eq!(app.view().cursor_source_line(), Some(1));
    let hunk = app
        .view()
        .hunks()
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("hunk"))?;
    let rows = app
        .view()
        .hunk_rows(&hunk, true)
        .ok_or_else(|| anyhow::anyhow!("old-side rows"))?;
    assert_eq!(app.view().rendered_row_of_source_line(1), Some(rows.start));
    assert_eq!(
        app.view().scroll(),
        0,
        "top clamp keeps the first old line visible"
    );
    Ok(())
}

fn assert_draft_cursor_visible(app: &App) -> anyhow::Result<()> {
    let (row, _) = app.draft_cursor_cell().context("draft cursor")?;
    let scroll = app.view().scroll();
    anyhow::ensure!(row >= scroll, "draft cursor {row} is above scroll {scroll}");
    anyhow::ensure!(
        row + 1 < scroll + app.text_rows(),
        "draft cursor {row} is hidden by the footer at scroll {scroll}"
    );
    Ok(())
}

#[test]
fn active_draft_owns_viewport_across_resize_and_delayed_highlight() -> anyhow::Result<()> {
    let base = (1..=90)
        .map(|line| format!("let value_{line} = {line};"))
        .collect::<Vec<_>>()
        .join("\n");
    let target = base.replace("let value_45 = 45;", "let value_45 = 4500;");
    let dir = TempDir::new("navigation-draft-viewport")?;
    fs::write(dir.0.join("main.rs"), &base)?;
    git::init(&dir.0)?;
    git::commit_and_stage(&dir.0, &[("main.rs", &base)])?;
    fs::write(dir.0.join("main.rs"), &target)?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let highlighter = Arc::new(Highlighter::new("base16-ocean.dark")?);
    let mut app = AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options.highlighter = highlighter;
            options
        })
        .build()?;
    app.open(Path::new("main.rs"));
    let current = app.current.context("open document")?;
    app.queue_highlight(current);
    let completed = app
        .take_highlight_jobs()
        .pop()
        .context("pending source highlight")?
        .complete();

    app.hunk_next();
    let identity = app
        .change_stop
        .as_ref()
        .map(|stop| (stop.old.clone(), stop.new.clone()))
        .context("comparison destination")?;
    app.start_new_comment();
    let range = match app.draft().context("draft")?.target() {
        crate::app::threads::ComposeTarget::New(range) => *range,
        other => anyhow::bail!("unexpected draft target: {other:?}"),
    };
    let width = app.draft_width();
    app.set_compose_text(&"draft text ".repeat(width * 3));
    assert_draft_cursor_visible(&app)?;

    app.resize(54, 12);
    assert_draft_cursor_visible(&app)?;
    assert_eq!(
        app.change_stop
            .as_ref()
            .map(|stop| (stop.old.clone(), stop.new.clone())),
        Some(identity.clone())
    );
    assert_eq!(
        app.draft().and_then(|compose| match compose.target() {
            crate::app::threads::ComposeTarget::New(current) => Some(*current),
            _ => None,
        }),
        Some(range)
    );

    assert!(app.apply_highlight(completed));
    assert_draft_cursor_visible(&app)?;
    assert_eq!(
        app.change_stop
            .as_ref()
            .map(|stop| (stop.old.clone(), stop.new.clone())),
        Some(identity)
    );

    app.compose_cancel();
    app.compose_cancel();
    app.resize(64, 14);
    let hunk = app.view().hunks().into_iter().next().context("hunk")?;
    let rows = app.view().hunk_rows(&hunk, false).context("hunk rows")?;
    assert_eq!(
        app.view().scroll() + (app.view().body_height() - 1) / 3,
        rows.start,
        "passive comparison placement resumes after draft cancellation"
    );
    Ok(())
}

#[test]
fn projected_normal_deletion_draft_keeps_its_cursor_visible_on_resize() -> anyhow::Result<()> {
    let (_dir, mut app) =
        deletion_and_later_change("navigation-deletion-draft-viewport", DiffMode::Normal)?;
    app.hunk_next();
    let identity = app
        .change_stop
        .as_ref()
        .map(|stop| (stop.old.clone(), stop.new.clone()))
        .context("deletion destination")?;
    let seat = app
        .view()
        .projected_old_seat()
        .context("projected deletion seat")?;
    app.start_new_comment();
    let width = app.draft_width();
    app.set_compose_text(&"deletion draft ".repeat(width * 3));

    app.resize(30, 10);
    assert_draft_cursor_visible(&app)?;
    assert_eq!(
        app.change_stop
            .as_ref()
            .map(|stop| (stop.old.clone(), stop.new.clone())),
        Some(identity)
    );
    assert_eq!(
        app.view().projected_old_seat(),
        Some(seat),
        "draft relayout preserves the projected old-side identity"
    );
    Ok(())
}

#[test]
fn unreadable_comparison_content_keeps_a_path_stop() -> anyhow::Result<()> {
    let dir = TempDir::new("navigation-unavailable-content")?;
    let root = &dir.0;
    fs::write(root.join("README.md"), "unchanged\n")?;
    git::init(root)?;
    git::commit_and_stage(root, &[("README.md", "unchanged\n")])?;
    fs::write(root.join("unavailable.txt"), [0xff, 0xfe])?;

    let mut app = AppBuilder::at(root).unopened().build()?;
    assert_eq!(app.comparison_status().entries().len(), 1);
    app.hunk_next();
    assert_eq!(app.current_path(), Path::new("unavailable.txt"));
    assert!(app.info().is_some(), "unavailable text uses file info");
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), None);
    assert!(
        app.info()
            .is_some_and(|info| info.notice.join(" ").contains("not UTF-8")),
        "{:?}",
        app.info()
    );
    Ok(())
}

#[test]
fn comparison_navigation_reveals_nested_destination_without_taking_files_focus()
-> anyhow::Result<()> {
    let dir = TempDir::new("navigation-tree-nested")?;
    let root = &dir.0;
    fs::create_dir_all(root.join("docs/deep"))?;
    fs::write(root.join("README.md"), "readme\n")?;
    fs::write(root.join("docs/deep/target.md"), "old\n")?;
    git::init(root)?;
    git::commit_and_stage(
        root,
        &[("README.md", "readme\n"), ("docs/deep/target.md", "old\n")],
    )?;
    fs::write(root.join("docs/deep/target.md"), "new\n")?;

    let mut app = AppBuilder::at(root).source_view().build()?;
    app.show_tree();
    assert_eq!(app.focus(), Focus::Tree);
    assert!(
        app.tree()
            .is_some_and(|tree| !tree.contains(Path::new("docs/deep/target.md")))
    );

    testing::press(&mut app, "J");

    assert_eq!(app.current_path(), Path::new("docs/deep/target.md"));
    assert_eq!(app.focus(), Focus::View);
    let tree = app.tree().context("visible files tree")?;
    assert_eq!(
        tree.current().map(fathomable_core::tree::Row::path),
        Some(Path::new("docs/deep/target.md"))
    );
    for path in ["docs", "docs/deep"] {
        assert!(
            tree.rows()
                .iter()
                .any(|row| row.path() == Path::new(path) && row.expanded()),
            "{path} should be expanded"
        );
    }

    app.toggle_tree_focus();
    testing::press(&mut app, "hh");
    app.refresh_review_paths();
    let tree = app.tree().context("visible files tree")?;
    assert_eq!(
        tree.current().map(fathomable_core::tree::Row::path),
        Some(Path::new("docs/deep"))
    );
    assert!(
        tree.current().is_some_and(|row| !row.expanded()),
        "manual collapse must survive passive refresh"
    );
    assert!(!tree.contains(Path::new("docs/deep/target.md")));
    Ok(())
}

#[test]
fn hidden_files_retains_filtered_destination_and_centers_it_when_admitted() -> anyhow::Result<()> {
    let dir = TempDir::new("navigation-tree-hidden")?;
    let root = &dir.0;
    let mut files = vec![("README.md".to_owned(), "readme\n".to_owned())];
    for index in 0..=40 {
        files.push((format!("{index:02}.md"), format!("old {index}\n")));
    }
    for (path, text) in &files {
        fs::write(root.join(path), text)?;
    }
    git::init(root)?;
    let refs: Vec<_> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    git::commit_and_stage(root, &refs)?;
    fs::write(root.join("20.md"), "new 20\n")?;

    let mut app = AppBuilder::at(root).source_view().build()?;
    app.resize(100, 12);
    app.show_tree();
    app.toggle_tree_shown();
    assert!(!app.sidebar.tree);

    testing::press(&mut app, "J");
    assert_eq!(app.current_path(), Path::new("20.md"));
    assert_eq!(app.focus(), Focus::View);
    app.toggle_tree_shown();

    let tree = app.tree().context("reopened files tree")?;
    let cursor = tree.cursor();
    assert_eq!(
        tree.current().map(fathomable_core::tree::Row::path),
        Some(Path::new("20.md"))
    );
    let body_rows = app.tree_rows().saturating_sub(1).max(1);
    assert_eq!(cursor - app.tree_scroll(), body_rows / 2);

    app.open(Path::new("README.md"));
    app.files_toggle(Rule::Reviews);
    app.toggle_tree_shown();
    testing::press(&mut app, "J");
    app.toggle_tree_shown();
    assert!(
        app.tree()
            .is_some_and(|tree| !tree.contains(Path::new("20.md"))),
        "the active filter must not fabricate the destination"
    );

    app.files_toggle(Rule::Reviews);
    assert_eq!(
        app.tree()
            .and_then(Tree::current)
            .map(fathomable_core::tree::Row::path),
        Some(Path::new("20.md")),
        "removing the filter retries the remembered destination"
    );
    assert_eq!(app.focus(), Focus::View);
    Ok(())
}

#[test]
fn main_threads_tab_stays_in_current_view() -> anyhow::Result<()> {
    let dir = testing::workspace("navigation-thread-source", testing::README)?;
    let root = testing::root(&dir);
    fs::create_dir_all(root.join("docs/deep"))?;
    fs::write(root.join("docs/deep/guide.md"), "guide\n")?;
    let mut store = Store::open(testing::store_path(&dir))?;
    store.annotate(
        testing::at_working_tree(
            &root,
            Draft::new(
                Author::agent("reviewer"),
                Path::new("README.md"),
                LineRange::new(1, 1),
                "readme",
            ),
            testing::README,
        )?,
        testing::README,
        1,
    )?;
    let destination = store.annotate(
        testing::at_working_tree(
            &root,
            Draft::new(
                Author::agent("reviewer"),
                Path::new("docs/deep/guide.md"),
                LineRange::new(1, 1),
                "guide",
            ),
            "guide\n",
        )?,
        "guide\n",
        1,
    )?;

    let mut app = AppBuilder::new(&dir).source_view().build()?;
    app.show_tree();
    app.open_review();
    assert_eq!(app.focus(), Focus::Review);

    testing::press_key(&mut app, crossterm::event::KeyCode::Tab);

    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.focus(), Focus::Review);
    assert!(app.review_list().is_open());
    assert_eq!(app.thread_cursor().thread(), Some(&destination));
    Ok(())
}

#[test]
fn one_thread_wrap_peeks_hidden_stub_and_selects_newest_reply() -> anyhow::Result<()> {
    let dir = testing::workspace("navigation-thread-wrap", testing::README)?;
    let root = testing::root(&dir);
    let mut store = Store::open(testing::store_path(&dir))?;
    let id = store.annotate(
        testing::at_working_tree(
            &root,
            Draft::new(
                Author::agent("reviewer"),
                Path::new("README.md"),
                LineRange::new(3, 4),
                "opening",
            ),
            testing::README,
        )?,
        testing::README,
        1,
    )?;
    store.reply(&id, Reply::new(Author::User, 2, "newest reply"))?;
    let mut app = AppBuilder::new(&dir).source_view().build()?;
    app.resize(80, 14);
    app.toggle_stubs();
    assert!(!app.stubs_shown());

    app.thread_step_across(1);

    assert_eq!(app.thread_cursor().thread(), Some(&id));
    assert_eq!(app.thread_cursor().message(), 1);
    assert!(app.is_expanded(&id), "the selected hidden stub peeks open");
    assert!(
        !app.expanded.contains(&id),
        "navigation does not replace the persistent fold"
    );
    assert_eq!(
        app.expanded_row_message(app.view().cursor().row),
        Some((id.clone(), 1)),
        "the actual File cursor sits on the newest reply"
    );

    app.set_thread_cursor_message(id.clone(), 0);
    app.thread_step_across(1);
    assert_eq!(
        app.thread_cursor().message(),
        1,
        "same-thread wrap resets to newest"
    );
    app.fold_thread(&id);
    assert!(!app.is_expanded(&id), "explicit folding owns visible state");
    Ok(())
}

#[test]
fn comparison_jump_places_changed_row_at_exact_top_third() -> anyhow::Result<()> {
    let base = (1..=30)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let dir = testing::workspace("navigation-third", &base)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", &base)])?;
    fs::write(
        root.join("README.md"),
        base.replace("line 20", "changed 20"),
    )?;
    let mut app = AppBuilder::new(&dir).source_view().build()?;
    app.resize(80, 12);
    app.view_mut().goto_source_line(1);

    app.hunk_next();

    let offset = app.view().cursor().row - app.view().scroll();
    assert_eq!(offset, (app.view().body_height() - 1) / 3);
    Ok(())
}

#[test]
fn local_file_motion_retires_comparison_placement_before_resize() -> anyhow::Result<()> {
    let base = (1..=40)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let dir = testing::workspace("navigation-local-motion", &base)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", &base)])?;
    fs::write(
        root.join("README.md"),
        base.replace("line 20", "changed 20"),
    )?;
    let mut app = AppBuilder::new(&dir).source_view().build()?;
    app.resize(80, 12);

    testing::press(&mut app, "J");
    let jump_row = app.view().cursor().row;
    testing::press(&mut app, "j");
    let local_row = app.view().cursor().row;
    assert!(local_row > jump_row);
    assert!(app.change_placement.is_none());
    assert!(
        app.change_stop.is_some(),
        "local motion retires placement without discarding traversal identity"
    );

    app.resize(70, 14);

    assert_eq!(app.view().cursor().row, local_row);
    assert_ne!(
        app.view().scroll() + (app.view().body_height() - 1) / 3,
        jump_row,
        "resize must not resurrect the retired J placement"
    );
    Ok(())
}

#[test]
fn local_motion_within_normal_deletion_advances_then_wraps() -> anyhow::Result<()> {
    let (_dir, mut app) =
        deletion_and_later_change("navigation-deletion-local-next", DiffMode::Normal)?;

    testing::press(&mut app, "J");
    let first = app.view().hunks()[0].clone();
    let first_rows = app
        .view()
        .hunk_rows(&first, false)
        .context("first hunk rows")?;
    assert_eq!(app.view().cursor().row, first_rows.start);
    assert!(
        first_rows.len() > 1,
        "fixture must expose old-side movement"
    );

    testing::press(&mut app, "j");
    let local_row = app.view().cursor().row;
    assert!(first_rows.contains(&local_row));
    assert!(
        app.view().layout().lines()[local_row]
            .diff_old_line()
            .is_some_and(|line| (20..=22).contains(&line)),
        "local movement remains on a projected old-side row"
    );
    assert!(app.change_placement.is_none());

    testing::press(&mut app, "J");
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), Some(1));
    assert_ne!(app.message(), Some("wrapped to first change"));

    testing::press(&mut app, "J");
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), Some(0));
    assert_eq!(app.message(), Some("wrapped to first change"));
    Ok(())
}

#[test]
fn wheel_then_change_jump_advances_in_unified_mode_and_reverse_wraps() -> anyhow::Result<()> {
    let (_dir, mut app) =
        deletion_and_later_change("navigation-deletion-wheel-next", DiffMode::Unified)?;

    testing::press(&mut app, "J");
    let first_cursor = app.view().cursor().row;
    let column = u16::try_from(app.sidebar_width() + 1)?;
    let row = u16::try_from(app.text_top())?;
    crate::app::input::mouse::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(app.view().cursor().row, first_cursor);
    assert!(app.change_placement.is_none());

    testing::press(&mut app, "J");
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), Some(1));
    assert_ne!(app.message(), Some("wrapped to first change"));

    testing::press(&mut app, "K");
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), Some(0));
    testing::press(&mut app, "j");
    testing::press(&mut app, "K");
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), Some(1));
    assert_eq!(app.message(), Some("wrapped to last change"));
    Ok(())
}

#[test]
fn normal_partial_deletion_projects_the_selected_old_side_rows() -> anyhow::Result<()> {
    let base = (1..=20)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let target = (1..=20)
        .filter(|line| !(8..=10).contains(line))
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let dir = testing::workspace("navigation-partial-deletion", &base)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", &base)])?;
    fs::write(root.join("README.md"), target)?;
    let mut app = AppBuilder::new(&dir).source_view().build()?;

    app.hunk_next();

    assert!(!app.view().diff_view());
    let hunk = app.view().hunks().into_iter().next().context("hunk")?;
    assert!(hunk.new_range().is_empty());
    let rows = app.view().hunk_rows(&hunk, false).context("old rows")?;
    assert_eq!(app.view().cursor().row, rows.start);
    assert_eq!(
        app.view().layout().lines()[rows.start].diff_old_line(),
        Some(8)
    );
    assert_eq!(rows.len(), 3);
    Ok(())
}

#[test]
fn jumplist_restores_exact_projected_deletion_seat_and_base_origin() -> anyhow::Result<()> {
    let (_dir, mut app) =
        deletion_and_later_change("navigation-deletion-jumplist", DiffMode::Normal)?;

    testing::press(&mut app, "J");
    testing::press(&mut app, "lllllll");
    let deletion_seat = app
        .view()
        .projected_old_seat()
        .context("projected deletion seat")?;
    assert_eq!(
        app.view().layout().lines()[app.view().cursor().row].diff_old_line(),
        Some(20)
    );

    testing::press(&mut app, "J");
    assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), Some(1));
    assert!(!app.view().projects_normal_deletion());
    let later_line = app
        .view()
        .cursor_source_line()
        .context("later target-side line")?;

    handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(app.view().projected_old_seat(), Some(deletion_seat));
    assert_eq!(
        app.view().layout().lines()[app.view().cursor().row].diff_old_line(),
        Some(20)
    );

    app.resize(44, 15);
    assert_eq!(
        app.view().projected_old_seat(),
        Some(deletion_seat),
        "resize retains old line and within-line cell"
    );

    handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
    assert_eq!(app.view().cursor_source_line(), Some(later_line));
    assert!(!app.view().projects_normal_deletion());

    handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::ALT));
    assert_eq!(app.view().projected_old_seat(), Some(deletion_seat));
    app.start_new_comment();
    app.compose_insert("old-side finding");
    app.compose_submit();
    let thread = app
        .store
        .as_ref()
        .and_then(|store| store.threads().first())
        .context("old-side thread")?;
    assert_eq!(thread.origin_side(), OriginSide::Base);
    assert_eq!(thread.origin().range(), Some(LineRange::new(20, 20)));
    assert_eq!(
        thread.origin().snippet(),
        format!("line 20 {}", "wrapped deletion ".repeat(8))
    );
    Ok(())
}

#[test]
fn normal_empty_retained_file_projects_all_deleted_rows() -> anyhow::Result<()> {
    let base = "first\nsecond\nthird\n";
    let dir = testing::workspace("navigation-empty-retained", base)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", base)])?;
    fs::write(root.join("README.md"), "")?;
    let mut app = AppBuilder::new(&dir).source_view().build()?;

    app.hunk_next();

    assert!(!app.view().diff_view());
    let hunk = app.view().hunks().into_iter().next().context("hunk")?;
    assert!(hunk.new_range().is_empty());
    let rows = app.view().hunk_rows(&hunk, false).context("old rows")?;
    assert_eq!(app.view().cursor().row, rows.start);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        app.view().layout().lines()[rows.start].diff_old_line(),
        Some(1)
    );
    Ok(())
}

#[test]
fn normal_reload_removes_an_obsolete_projected_deletion() -> anyhow::Result<()> {
    let base = "a\nb\nc\nd\n";
    let target = "a\nd\n";
    let dir = testing::workspace("navigation-reload-deletion", base)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", base)])?;
    fs::write(root.join("README.md"), target)?;
    let mut app = AppBuilder::new(&dir).source_view().build()?;

    app.hunk_next();
    assert!(
        app.view()
            .layout()
            .lines()
            .iter()
            .any(|line| line.diff_old_line().is_some())
    );

    assert!(app.view_mut().reload(base.to_owned()));

    assert!(app.view().hunks().is_empty());
    assert!(
        app.view()
            .layout()
            .lines()
            .iter()
            .all(|line| line.diff_old_line().is_none())
    );
    assert_eq!(
        app.view()
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .collect::<Vec<_>>(),
        ["a", "b", "c", "d"]
    );
    Ok(())
}

#[test]
fn normal_eof_deletion_is_appended_after_the_last_survivor() -> anyhow::Result<()> {
    let base = "a\nb\nc\n";
    let target = "a\nb\n";
    let dir = testing::workspace("navigation-eof-deletion", base)?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", base)])?;
    fs::write(root.join("README.md"), target)?;
    let mut app = AppBuilder::new(&dir).source_view().build()?;

    app.hunk_next();

    let rows = app
        .view()
        .layout()
        .lines()
        .iter()
        .map(fathomable_core::layout::Line::text)
        .collect::<Vec<_>>();
    assert_eq!(rows, ["a", "b", "-c"]);
    assert_eq!(app.view().cursor().row, 2);
    assert_eq!(
        app.view().layout().lines()[2].diff_old_line(),
        Some(3),
        "the projected deletion occupies the append boundary"
    );
    Ok(())
}

#[test]
fn review_fallback_reveals_thread_file_after_hidden_files_reopens() -> anyhow::Result<()> {
    let base = (1..=20)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let dir = testing::workspace("navigation-thread-fallback", &base)?;
    let root = testing::root(&dir);
    fs::write(root.join("binary.bin"), [0, 1, 2])?;
    git::init(&root)?;
    git::commit_and_stage(
        &root,
        &[("README.md", &base), ("binary.bin", "\0\u{1}\u{2}")],
    )?;
    fs::write(
        root.join("README.md"),
        base.replace("line 5", "changed 5")
            .replace("line 15", "changed 15"),
    )?;
    let mut store = Store::open(testing::store_path(&dir))?;
    store.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("binary.bin"),
            LineRange::new(1, 1),
            "binary",
        ),
        "source\n",
        1,
    )?;

    let mut app = AppBuilder::new(&dir).source_view().build()?;
    app.show_tree();
    app.toggle_tree_shown();
    app.open_review();
    assert_eq!(app.focus(), Focus::Review);

    testing::press_key(&mut app, crossterm::event::KeyCode::Tab);

    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.focus(), Focus::Review);
    assert!(app.review_list().is_open());
    app.toggle_tree_shown();
    assert_eq!(app.focus(), Focus::Review);
    assert_eq!(
        app.tree()
            .and_then(Tree::current)
            .map(fathomable_core::tree::Row::path),
        Some(Path::new("binary.bin"))
    );

    testing::press(&mut app, "J");
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.focus(), Focus::View);
    assert_eq!(
        app.tree()
            .and_then(Tree::current)
            .map(fathomable_core::tree::Row::path),
        Some(Path::new("README.md")),
        "same-file hunk landing replaces the fallback destination"
    );
    Ok(())
}
