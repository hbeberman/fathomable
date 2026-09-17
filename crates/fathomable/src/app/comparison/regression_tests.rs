use std::fs;
use std::path::Path;

use fathomable_testing::{TempDir, git};

use crate::app::testing::{AppBuilder, screen};
use crate::app::{ComparisonSide, PickerKind, Popup};
use fathomable_core::workspace::{ComparisonEndpoint, Workspace};

fn repository(name: &str) -> anyhow::Result<TempDir> {
    let dir = TempDir::new(name)?;
    fs::create_dir_all(dir.0.join("ws"))?;
    git::init(&dir.0.join("ws"))?;
    Ok(dir)
}

fn picker_items(app: &crate::app::App) -> Vec<String> {
    match app.popup() {
        Some(Popup::Picker(picker)) => picker
            .matches()
            .iter()
            .map(|item| picker.item(item).to_owned())
            .collect(),
        _ => Vec::new(),
    }
}

#[test]
fn comparison_picker_drills_into_tags_branches_and_commits() -> anyhow::Result<()> {
    let dir = repository("comparison-picker-menus")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let head_hex = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
    git::tag(&root, "v1")?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .review_points(dir.0.join("points"))
        .build()?;

    app.open_picker(PickerKind::ComparisonBase);
    let items = picker_items(&app);
    assert_eq!(
        &items[..7],
        [
            "Working tree",
            "Index",
            "HEAD",
            "Tags...",
            "Branches...",
            "Review points...",
            "Advanced...",
        ]
    );
    assert!(super::commit_id_from_row(&items[7]).is_some());
    let rendered = screen(&app)?;
    assert!(rendered.iter().any(|line| line.contains("Working tree")));
    assert!(
        rendered
            .iter()
            .any(|line| line.contains(&head_hex[..7]) && line.contains("1970-01-01"))
    );
    assert!(
        rendered.iter().all(|line| !line.contains(&head_hex)),
        "the picker displays short commit IDs"
    );

    app.open_picker(PickerKind::ComparisonBase);
    app.picker_move(3);
    app.picker_confirm();
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker))
            if picker.kind() == PickerKind::ComparisonTags(ComparisonSide::Base)
    ));
    assert_eq!(picker_items(&app), ["tag v1"]);
    app.picker_confirm();
    assert_eq!(app.comparison.base().to_string(), head_hex[..7]);

    app.open_picker(PickerKind::ComparisonBase);
    app.picker_move(4);
    app.picker_confirm();
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker))
            if picker.kind() == PickerKind::ComparisonBranches(ComparisonSide::Base)
    ));
    assert_eq!(picker_items(&app), ["branch main"]);
    app.picker_confirm();
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker))
            if picker.kind() == PickerKind::ComparisonBranchCommits(ComparisonSide::Base)
                && picker.scope() == Some("main")
    ));
    assert!(super::commit_id_from_row(&picker_items(&app)[0]).is_some());
    app.picker_confirm();
    assert!(app.popup().is_none());

    app.open_picker(PickerKind::ComparisonBase);
    app.picker_move(4);
    app.picker_confirm();
    app.picker_confirm();
    app.picker_escape();
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker))
            if picker.kind() == PickerKind::ComparisonBranches(ComparisonSide::Base)
    ));
    app.picker_escape();
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.kind() == PickerKind::ComparisonBase
    ));
    Ok(())
}

#[test]
fn comparison_picker_distinguishes_index_from_head() -> anyhow::Result<()> {
    let dir = repository("comparison-picker-fixed-endpoints")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let head = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .review_points(dir.0.join("points"))
        .build()?;

    app.open_picker(PickerKind::ComparisonTarget);
    assert!(!picker_items(&app).contains(&"Review points...".to_owned()));
    app.picker_move(1);
    app.picker_confirm();
    assert_eq!(app.comparison.target(), &ComparisonEndpoint::Index);

    app.open_picker(PickerKind::ComparisonTarget);
    app.picker_move(2);
    app.picker_confirm();
    assert!(matches!(
        app.comparison.target(),
        ComparisonEndpoint::Commit(id) if id.as_str() == head
    ));
    Ok(())
}

#[test]
fn four_hex_characters_search_beyond_loaded_commit_rows() -> anyhow::Result<()> {
    let dir = repository("comparison-picker-id-search")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let head = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.open_scoped_picker(
        PickerKind::ComparisonTarget,
        vec!["Working tree".to_owned()],
        None,
    );

    for ch in head[..4].chars() {
        app.picker_char(ch);
    }
    let items = picker_items(&app);
    assert!(
        items
            .iter()
            .any(|item| super::commit_id_from_row(item) == Some(head.as_str()))
    );
    Ok(())
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
