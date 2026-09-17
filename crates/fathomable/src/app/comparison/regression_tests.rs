use std::fs;
use std::path::Path;

use fathomable_testing::{TempDir, git};

use crate::app::testing::AppBuilder;

fn repository(name: &str) -> anyhow::Result<TempDir> {
    let dir = TempDir::new(name)?;
    fs::create_dir_all(dir.0.join("ws"))?;
    git::init(&dir.0.join("ws"))?;
    Ok(dir)
}

#[test]
fn failed_mutable_refresh_keeps_the_last_good_diff() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = repository("comparison-stale-presentation")?;
    let root = dir.0.join("ws");
    fs::write(root.join("a.txt"), "one\n")?;
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    fs::write(root.join("a.txt"), "two\n")?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.open(Path::new("a.txt"));
    app.show_comparison_diff();
    let before: Vec<String> = app
        .view()
        .layout()
        .lines()
        .iter()
        .map(fathomable_core::layout::Line::text)
        .collect();

    let file = root.join("a.txt");
    let original_mode = fs::metadata(&file)?.permissions().mode();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o0))?;
    app.refresh_comparison();
    fs::set_permissions(&file, fs::Permissions::from_mode(original_mode))?;

    assert!(app.comparison.stale());
    assert_eq!(
        app.view()
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .collect::<Vec<_>>(),
        before
    );
    assert_eq!(
        app.view().diff().map(|diff| diff.badge.as_str()),
        Some("DIFF stale")
    );
    fs::write(&file, "three\n")?;
    let index = app.current.ok_or_else(|| anyhow::anyhow!("no document"))?;
    assert!(app.reload_doc(index).is_some());
    app.leave_diff();
    app.show_comparison_diff();
    assert_eq!(
        app.view()
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .collect::<Vec<_>>(),
        before,
        "watcher reloads must not mix new bytes into a stale comparison"
    );
    Ok(())
}

#[test]
fn deleting_an_open_untracked_file_clears_its_cached_diff() -> anyhow::Result<()> {
    let dir = repository("comparison-deleted-untracked")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("tracked.txt", "tracked\n")])?;
    fs::write(root.join("scratch.txt"), "untracked\n")?;

    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.open(Path::new("scratch.txt"));
    app.show_comparison_diff();
    assert_eq!(app.view().pair_counts(), Some((1, 0)));

    fs::remove_file(root.join("scratch.txt"))?;
    app.on_removed(Path::new("scratch.txt"));
    app.refresh_comparison();
    app.leave_diff();
    app.show_comparison_diff();

    assert!(!app.comparison.stale());
    assert_eq!(app.view().pair_counts(), Some((0, 0)));
    Ok(())
}
