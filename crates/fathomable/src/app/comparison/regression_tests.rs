use std::fs;
use std::path::Path;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fathomable_core::annotations::{Author, Draft, LineRange, Store};
use fathomable_core::config::DiffMode;
use fathomable_testing::{TempDir, git};

use crate::app::draw;
use crate::app::input::mouse::handle_mouse;
use crate::app::testing::{AppBuilder, buffer, screen};
use crate::app::{ComparisonSide, Focus, PickerKind, Popup};
use fathomable_core::theme::Theme as CoreTheme;
use fathomable_core::workspace::{CommitId, ComparisonEndpoint, Workspace};

use super::EndpointAlias;

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

fn menu_app(root: &Path, points: &Path) -> anyhow::Result<crate::app::App> {
    AppBuilder::at(root)
        .unopened()
        .review_points(points)
        .options(|mut options| {
            options.menu_bar = true;
            options
        })
        .build()
}

fn text_cell(
    screen: &[String],
    buffer: &ratatui::buffer::Buffer,
    row_text: &str,
    text: &str,
) -> Option<ratatui::buffer::Cell> {
    let (row, line) = screen
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains(row_text) && line.contains(text))?;
    let column = line.find(text)?;
    Some(buffer[(u16::try_from(column).ok()?, u16::try_from(row).ok()?)].clone())
}

fn mouse(app: &mut crate::app::App, kind: MouseEventKind, column: usize, row: usize) {
    handle_mouse(
        app,
        MouseEvent {
            kind,
            column: u16::try_from(column).unwrap_or(u16::MAX),
            row: u16::try_from(row).unwrap_or(u16::MAX),
            modifiers: KeyModifiers::NONE,
        },
    );
}

#[test]
fn comparison_picker_marks_current_endpoints_before_dates() -> anyhow::Result<()> {
    let dir = repository("comparison-picker-current-endpoints")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let head = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(|mut options| {
            options.menu_bar = true;
            options
        })
        .build()?;

    app.open_picker(PickerKind::ComparisonBase);
    let rendered = screen(&app)?;
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("Working tree") && line.contains("[current target]"))
    );
    assert!(
        rendered
            .iter()
            .any(|line| line.contains("HEAD") && line.contains("[current base]"))
    );
    let commit = rendered
        .iter()
        .find(|line| line.contains(&head[..7]) && line.contains("[current base]"))
        .ok_or_else(|| anyhow::anyhow!("current commit row"))?;
    assert!(
        commit.find("[current base]") < commit.find("1970-01-01"),
        "the endpoint badge stays left of the date"
    );
    let cells = buffer(&app)?;
    let base = text_cell(&rendered, &cells, "HEAD", "[current base]")
        .ok_or_else(|| anyhow::anyhow!("base badge cell"))?;
    let target = text_cell(&rendered, &cells, "Working tree", "[current target]")
        .ok_or_else(|| anyhow::anyhow!("target badge cell"))?;
    let theme = draw::Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
    assert_eq!(base.fg, target.fg, "both endpoint badges share one accent");
    assert_eq!(Some(base.fg), theme.popup_key.fg);
    assert!(
        rendered.iter().all(|line| !line.contains(&head)),
        "the picker displays short commit IDs"
    );
    Ok(())
}

#[test]
fn selecting_an_endpoint_refreshes_the_comparison_once() -> anyhow::Result<()> {
    let dir = repository("comparison-picker-single-refresh")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let head = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
    fs::write(root.join("a.txt"), "two\n")?;
    let mut app = menu_app(&root, &dir.0.join("points"))?;
    let generation = app.comparison.generation();

    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&head)?));

    assert_eq!(app.comparison.generation(), generation.wrapping_add(1));
    Ok(())
}

#[test]
fn menu_bar_endpoint_buttons_hover_and_open_their_pickers() -> anyhow::Result<()> {
    let dir = repository("comparison-menu-buttons")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let mut app = menu_app(&root, &dir.0.join("points"))?;
    let tail = crate::app::menu_bar::bar_tail(&app, app.size().0);
    let base = tail
        .base
        .ok_or_else(|| anyhow::anyhow!("visible base button"))?;
    let target = tail
        .target
        .ok_or_else(|| anyhow::anyhow!("visible target button"))?;

    mouse(&mut app, MouseEventKind::Moved, base.x, 0);
    let cells = buffer(&app)?;
    let theme = draw::Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
    let hovered = &cells[(u16::try_from(base.x)?, 0)];
    assert_eq!(Some(hovered.bg), theme.list_hover.bg);
    assert_eq!(Some(hovered.fg), theme.popup_key.fg);

    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), base.x, 0);
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.kind() == PickerKind::ComparisonBase
    ));
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        target.x,
        0,
    );
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.kind() == PickerKind::ComparisonTarget
    ));
    Ok(())
}

#[test]
fn branch_picker_mouse_hovers_clicks_and_wheels() -> anyhow::Result<()> {
    let dir = repository("comparison-branch-picker-mouse")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let head = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
    let refs = root.join(".git/refs/heads");
    for index in 0..25 {
        fs::write(refs.join(format!("branch-{index:02}")), format!("{head}\n"))?;
    }
    let mut app = menu_app(&root, &dir.0.join("points"))?;
    app.toggle_sidebar();
    assert!(app.sidebar_width() > 0, "the Files pane is visible");
    app.open_picker(PickerKind::ComparisonBranches(ComparisonSide::Base));

    let Some(Popup::Picker(picker)) = app.popup() else {
        return Err(anyhow::anyhow!("branch picker"));
    };
    let layout = draw::picker_layout(&app, picker);
    let (width, height) = app.size();
    let frame = (0..height)
        .find_map(|row| {
            (0..width)
                .find(|&column| layout.contains(column, row))
                .map(|column| (column, row))
        })
        .ok_or_else(|| anyhow::anyhow!("picker frame"))?;
    assert_eq!(layout.entry_at(frame.0, frame.1, picker.matched()), None);
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        frame.0,
        frame.1,
    );
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker))
            if picker.kind() == PickerKind::ComparisonBranches(ComparisonSide::Base)
    ));

    let outside = (0, app.pane_top() + 1);
    assert!(!layout.contains(outside.0, outside.1));
    assert!(outside.0 < app.sidebar_width());
    assert!((1..app.tree_rows()).contains(&(outside.1 - app.pane_top())));
    mouse(&mut app, MouseEventKind::ScrollDown, outside.0, outside.1);
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.selected() == 0
    ));

    let rendered = screen(&app)?;
    let (row, line) = rendered
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains("branch branch-05"))
        .ok_or_else(|| anyhow::anyhow!("visible branch row"))?;
    let column = line
        .find("branch branch-05")
        .ok_or_else(|| anyhow::anyhow!("branch column"))?;
    mouse(&mut app, MouseEventKind::Moved, column, row);
    let cells = buffer(&app)?;
    let theme = draw::Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
    assert_eq!(
        Some(cells[(u16::try_from(column)?, u16::try_from(row)?)].bg),
        theme.list_hover.bg
    );

    mouse(&mut app, MouseEventKind::ScrollDown, column, row);
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.selected() == 3
    ));
    mouse(&mut app, MouseEventKind::ScrollUp, column, row);
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.selected() == 0
    ));

    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        column,
        row,
    );
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker))
            if picker.kind() == PickerKind::ComparisonBranchCommits(ComparisonSide::Base)
                && picker.scope() == Some("branch-05")
    ));

    app.focus_pane(Focus::View);
    mouse(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        outside.0,
        outside.1,
    );
    assert!(app.popup().is_none());
    assert_eq!(
        app.focus(),
        Focus::View,
        "the dismissed click does not focus the underlying Files pane"
    );
    Ok(())
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
    let points = dir.0.join("points");
    let mut app = menu_app(&root, &points)?;

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
    let tagged_screen = screen(&app)?;
    let bar = &tagged_screen[0];
    assert!(
        bar.trim_end().ends_with("Tag v1 to WorkingTree"),
        "the comparison pair ends the menu bar: {bar:?}"
    );
    drop(app);
    let mut app = menu_app(&root, &points)?;
    assert_eq!(
        app.comparison_menu_pair(),
        ("Tag v1".to_owned(), "WorkingTree".to_owned())
    );
    app.open_picker(PickerKind::ComparisonTags(ComparisonSide::Base));
    assert!(
        screen(&app)?
            .iter()
            .any(|line| line.contains("tag v1") && line.contains("[current base]"))
    );
    app.picker_escape();

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
    assert!(
        screen(&app)?
            .iter()
            .any(|line| line.contains("Index") && line.contains("[current target]"))
    );
    app.picker_move(2);
    app.picker_confirm();
    assert!(matches!(
        app.comparison.target(),
        ComparisonEndpoint::Commit(id) if id.as_str() == head
    ));
    Ok(())
}

#[test]
fn moved_tag_alias_falls_back_to_the_pinned_commit() -> anyhow::Result<()> {
    let dir = repository("comparison-picker-moved-tag")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no first commit"))?;
    git::tag(&root, "v1")?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.open_picker(PickerKind::ComparisonBase);
    app.picker_move(3);
    app.picker_confirm();
    app.picker_confirm();
    assert_eq!(app.comparison_menu_pair().0, "Tag v1");

    git::commit_and_stage(&root, &[("a.txt", "two\n")])?;
    git::retag(&root, "v1")?;
    app.refresh_comparison();

    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(fathomable_core::workspace::CommitId::parse(&first)?)
    );
    assert_eq!(app.comparison_menu_pair().0, first[..7]);
    Ok(())
}

#[test]
fn moved_target_tag_alias_falls_back_while_off() -> anyhow::Result<()> {
    let dir = repository("comparison-off-moved-target-tag")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("no first commit"))?;
    git::tag(&root, "v1")?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.select_diff_mode(DiffMode::Off);
    app.set_comparison_target_aliased(
        ComparisonEndpoint::Commit(CommitId::parse(&first)?),
        Some(EndpointAlias::Tag("v1".to_owned())),
    );
    assert_eq!(app.comparison_menu_pair().1, "Tag v1");

    git::commit_and_stage(&root, &[("a.txt", "two\n")])?;
    git::retag(&root, "v1")?;
    app.refresh_comparison();

    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert_eq!(app.comparison_menu_pair().1, first[..7]);
    assert_eq!(
        app.comparison.target(),
        &ComparisonEndpoint::Commit(CommitId::parse(&first)?)
    );
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
    app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
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
    app.select_diff_mode(fathomable_core::config::DiffMode::Standard);
    app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
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
    app.select_diff_mode(fathomable_core::config::DiffMode::Off);
    assert_eq!(app.diff_mode(), fathomable_core::config::DiffMode::Off);
    assert_eq!(app.view().text(), "three\n");
    assert!(!app.view().diff_view());
    assert!(
        app.view()
            .layout()
            .lines()
            .iter()
            .all(|line| !matches!(line.text().as_str(), "one" | "two")),
        "Off must discard stale unified and Base-derived text"
    );
    Ok(())
}

#[test]
fn off_clears_stale_unified_when_the_target_read_also_fails() -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = repository("comparison-off-failed-target")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "BASE_SENTINEL\n")])?;
    fs::write(root.join("a.txt"), "TARGET_SENTINEL\n")?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.open(Path::new("a.txt"));
    app.select_diff_mode(fathomable_core::config::DiffMode::Unified);

    let file = root.join("a.txt");
    let original_mode = fs::metadata(&file)?.permissions().mode();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o0))?;
    app.refresh_comparison();
    assert!(app.comparison.stale());
    app.select_diff_mode(fathomable_core::config::DiffMode::Off);
    fs::set_permissions(&file, fs::Permissions::from_mode(original_mode))?;

    assert_eq!(app.diff_mode(), fathomable_core::config::DiffMode::Off);
    assert!(!app.view().diff_view());
    assert_eq!(app.view().text(), "");
    assert!(
        app.view()
            .layout()
            .lines()
            .iter()
            .all(|line| !line.text().contains("SENTINEL")),
        "a failed Target read must not retain stale Base or Target diff rows"
    );
    Ok(())
}

#[test]
fn off_uses_only_target_with_review_point_or_unavailable_base() -> anyhow::Result<()> {
    let dir = repository("comparison-off-independent-base")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "BASE_SENTINEL\n")])?;
    fs::write(root.join("a.txt"), "REVIEW_POINT_SENTINEL\n")?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .review_points(dir.0.join("points"))
        .build()?;
    app.save_review_point(Some("captured"));
    let point = app
        .review_points
        .as_ref()
        .and_then(|store| store.list().into_iter().next())
        .map(|point| point.id().to_owned())
        .ok_or_else(|| anyhow::anyhow!("review point"))?;
    fs::write(root.join("a.txt"), "TARGET_SENTINEL\n")?;
    git::commit_and_stage(&root, &[("a.txt", "TARGET_SENTINEL\n")])?;
    let target = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("target commit"))?;

    app.open(Path::new("a.txt"));
    app.select_diff_mode(fathomable_core::config::DiffMode::Off);
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.set_comparison_base(ComparisonEndpoint::ReviewPoint(point));
    assert_eq!(
        app.diff_mode(),
        fathomable_core::config::DiffMode::Off,
        "a review-point Base cannot activate against a commit Target"
    );
    assert_eq!(app.view().text(), "TARGET_SENTINEL\n");

    let unavailable =
        ComparisonEndpoint::Commit(CommitId::parse("0000000000000000000000000000000000000000")?);
    app.set_comparison_base(unavailable);
    assert_eq!(app.diff_mode(), fathomable_core::config::DiffMode::Off);
    assert_eq!(app.view().text(), "TARGET_SENTINEL\n");
    assert!(!app.view().text().contains("BASE_SENTINEL"));
    assert!(!app.view().text().contains("REVIEW_POINT_SENTINEL"));

    app.set_comparison_base(ComparisonEndpoint::ReviewPoint("missing-point".to_owned()));
    assert_eq!(app.diff_mode(), fathomable_core::config::DiffMode::Off);
    assert_eq!(app.view().text(), "TARGET_SENTINEL\n");

    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    fs::write(root.join("a.txt"), "REFRESHED_TARGET_SENTINEL\n")?;
    app.refresh_comparison();
    assert_eq!(app.view().text(), "REFRESHED_TARGET_SENTINEL\n");
    Ok(())
}

#[test]
fn off_target_only_holds_across_recent_jumplist_and_review_jumps() -> anyhow::Result<()> {
    let dir = repository("comparison-off-navigation")?;
    let root = dir.0.join("ws");
    fs::write(root.join("gone.md"), "BASE_ONLY_SENTINEL\n")?;
    fs::write(root.join("stay.md"), "old\n")?;
    git::commit_and_stage(
        &root,
        &[("gone.md", "BASE_ONLY_SENTINEL\n"), ("stay.md", "old\n")],
    )?;
    let base = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("base commit"))?;
    fs::remove_file(root.join("gone.md"))?;
    fs::write(root.join("stay.md"), "TARGET_ONLY_SENTINEL\n")?;
    git::commit_and_stage(&root, &[("stay.md", "TARGET_ONLY_SENTINEL\n")])?;
    let target = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("target commit"))?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let thread = store.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("gone.md"),
            LineRange::new(1, 1),
            "historical evidence",
        )
        .at_commit(Some(base.clone())),
        "BASE_ONLY_SENTINEL\n",
        5,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&base)?));
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.open(Path::new("gone.md"));
    app.open(Path::new("stay.md"));
    app.select_diff_mode(fathomable_core::config::DiffMode::Off);

    app.open_picker(PickerKind::Recent);
    app.picker_move(1);
    app.picker_confirm();
    assert_target_absent_without_base(&app);

    app.open(Path::new("stay.md"));
    app.record_jump(crate::app::jumplist::Position::File {
        path: Path::new("gone.md").to_path_buf(),
        line: 1,
        thread: None,
    });
    app.jump_back();
    assert_target_absent_without_base(&app);

    app.open(Path::new("stay.md"));
    assert_eq!(
        app.land_on_thread(thread),
        Some(crate::app::threads::cursor::ThreadLanding::Review)
    );
    assert_eq!(app.current_path(), Path::new("stay.md"));
    assert_eq!(app.view().text(), "TARGET_ONLY_SENTINEL\n");
    assert!(
        app.review_list().is_open(),
        "deleted source falls back to Reviews"
    );
    assert_eq!(app.review_entries(false).len(), 1);
    Ok(())
}

fn assert_target_absent_without_base(app: &crate::app::App) {
    assert_eq!(app.current_path(), Path::new("gone.md"));
    assert_eq!(app.view().text(), "");
    assert_eq!(
        app.current
            .and_then(|index| app.docs[index].comparison_notice.as_deref()),
        Some("not present in Target")
    );
    assert!(
        app.view()
            .layout()
            .lines()
            .iter()
            .all(|line| !line.text().contains("BASE_ONLY_SENTINEL"))
    );
}

#[test]
fn deleting_an_open_untracked_file_clears_its_cached_diff() -> anyhow::Result<()> {
    let dir = repository("comparison-deleted-untracked")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("tracked.txt", "tracked\n")])?;
    fs::write(root.join("scratch.txt"), "untracked\n")?;

    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.open(Path::new("scratch.txt"));
    app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
    assert_eq!(app.view().pair_counts(), Some((1, 0)));

    fs::remove_file(root.join("scratch.txt"))?;
    app.on_removed(Path::new("scratch.txt"));
    app.refresh_comparison();
    app.select_diff_mode(fathomable_core::config::DiffMode::Standard);
    app.select_diff_mode(fathomable_core::config::DiffMode::Unified);

    assert!(!app.comparison.stale());
    assert_eq!(app.view().pair_counts(), Some((0, 0)));
    Ok(())
}
