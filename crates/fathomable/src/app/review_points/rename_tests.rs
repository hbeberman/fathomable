use std::fs;

use anyhow::Context as _;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use fathomable_core::annotations::Store;
use fathomable_core::review_points::{ReviewPoint, ReviewPointStore};
use fathomable_core::workspace::Workspace;
use fathomable_testing::{TempDir, git};

use crate::app::draw;
use crate::app::input::{keys, mouse};
use crate::app::testing::AppBuilder;
use crate::app::{App, PickerKind, Popup};

fn fixture(name: &str) -> anyhow::Result<(TempDir, std::path::PathBuf)> {
    let dir = TempDir::new(name)?;
    let root = dir.0.join("repo");
    fs::create_dir(&root)?;
    git::init(&root)?;
    git::commit_and_stage(&root, &[("README.md", "base\n")])?;
    Ok((dir, root))
}

fn capture(
    store: &mut ReviewPointStore,
    workspace: &mut Workspace,
    root: &std::path::Path,
    name: &str,
    text: &str,
) -> anyhow::Result<ReviewPoint> {
    fs::write(root.join("README.md"), text)?;
    Ok(store
        .capture(workspace, Some(name))?
        .point()
        .context("published point")?
        .clone())
}

fn select_point(app: &mut App, id: &str) -> anyhow::Result<()> {
    let index = match app.popup() {
        Some(Popup::Picker(picker)) => picker
            .matches()
            .iter()
            .position(|matched| {
                super::review_point_id_from_row(picker.kind(), picker.item(matched)) == Some(id)
            })
            .context("point row")?,
        _ => anyhow::bail!("point picker is not open"),
    };
    app.picker_select(index);
    Ok(())
}

fn selected_id(app: &App) -> Option<&str> {
    match app.popup() {
        Some(Popup::Picker(picker)) => {
            picker.matches().get(picker.selected()).and_then(|matched| {
                super::review_point_id_from_row(picker.kind(), picker.item(matched))
            })
        }
        _ => None,
    }
}

fn key(app: &mut App, code: KeyCode) {
    keys::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl_r(app: &mut App) {
    keys::handle_key(
        app,
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
    );
}

fn manager_rename(app: &mut App) {
    app.picker_confirm();
    key(app, KeyCode::Char('r'));
}

fn click(app: &mut App, column: usize, row: usize) {
    mouse::handle_mouse(
        app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: u16::try_from(column).unwrap_or(u16::MAX),
            row: u16::try_from(row).unwrap_or(u16::MAX),
            modifiers: KeyModifiers::NONE,
        },
    );
}

#[test]
fn manager_opens_action_card_and_rename_edits_prefilled_unicode_name() -> anyhow::Result<()> {
    let (dir, root) = fixture("review-point-manager-rename")?;
    let points = dir.0.join("points");
    let mut workspace = Workspace::discover(&root)?;
    let mut store = ReviewPointStore::open(&points)?;
    let point = capture(&mut store, &mut workspace, &root, "café", "first\n")?;
    let mut app = AppBuilder::at(&root).review_points(&points).build()?;

    app.request_review_point_manage();
    for character in "café".chars() {
        app.picker_char(character);
    }
    select_point(&mut app, point.id())?;
    ctrl_r(&mut app);
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.kind() == PickerKind::ReviewPointManage
    ));
    let layout = match app.popup() {
        Some(Popup::Picker(picker)) => draw::picker_layout(&app, picker),
        _ => anyhow::bail!("manager picker"),
    };
    assert!(
        !(0..app.pane_rows())
            .flat_map(|row| (0..app.size().0).map(move |column| (column, row)))
            .any(|(column, row)| layout.rename_at(column, row))
    );
    app.picker_confirm();
    assert!(matches!(
        app.popup(),
        Some(Popup::ReviewPointAction(action)) if action.point().id() == point.id()
    ));
    key(&mut app, KeyCode::Char('r'));
    assert!(matches!(
        app.popup(),
        Some(Popup::ReviewPointRename(rename)) if rename.editor().text() == "café"
    ));

    key(&mut app, KeyCode::Home);
    for _ in 0.."café".chars().count() {
        key(&mut app, KeyCode::Delete);
    }
    key(&mut app, KeyCode::Char('新'));
    key(&mut app, KeyCode::Char('名'));
    key(&mut app, KeyCode::Enter);

    assert_eq!(
        app.review_points
            .as_ref()
            .and_then(|store| store.get(point.id()))
            .and_then(ReviewPoint::name),
        Some("新名")
    );
    assert_eq!(selected_id(&app), Some(point.id()));
    assert!(
        app.toasts()
            .last()
            .is_some_and(|toast| toast.text().contains("Renamed review point"))
    );

    manager_rename(&mut app);
    key(&mut app, KeyCode::Home);
    key(&mut app, KeyCode::Delete);
    key(&mut app, KeyCode::Delete);
    key(&mut app, KeyCode::Enter);
    assert!(
        app.review_points
            .as_ref()
            .and_then(|store| store.get(point.id()))
            .is_some_and(|point| point.name().is_none())
    );
    assert_eq!(selected_id(&app), Some(point.id()));
    Ok(())
}

#[test]
fn returning_from_an_action_keeps_the_filter_and_stable_selection() -> anyhow::Result<()> {
    let (dir, root) = fixture("review-point-action-return")?;
    let points = dir.0.join("points");
    let mut workspace = Workspace::discover(&root)?;
    let mut external = ReviewPointStore::open(&points)?;
    let first = capture(
        &mut external,
        &mut workspace,
        &root,
        "shared first",
        "first\n",
    )?;
    let second = capture(
        &mut external,
        &mut workspace,
        &root,
        "shared second",
        "second\n",
    )?;
    let mut app = AppBuilder::at(&root).review_points(&points).build()?;

    app.request_review_point_manage();
    for character in "shared".chars() {
        app.picker_char(character);
    }
    let initially_selected = selected_id(&app);
    let target = [&first, &second]
        .into_iter()
        .find(|point| Some(point.id()) != initially_selected)
        .context("non-default filtered review point")?;
    select_point(&mut app, target.id())?;
    app.picker_confirm();
    assert!(matches!(app.popup(), Some(Popup::ReviewPointAction(_))));

    key(&mut app, KeyCode::Esc);

    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker))
            if picker.input() == "shared" && selected_id(&app) == Some(target.id())
    ));
    Ok(())
}

#[test]
fn rename_preserves_a_full_length_name_without_appending_a_manifest_line() -> anyhow::Result<()> {
    let (dir, root) = fixture("review-point-rename-full-length-noop")?;
    let points = dir.0.join("points");
    let manifest = points.join("review-points.jsonl");
    let name = "界".repeat(128);
    let mut workspace = Workspace::discover(&root)?;
    let mut store = ReviewPointStore::open(&points)?;
    let point = capture(&mut store, &mut workspace, &root, &name, "first\n")?;
    let before = fs::read(&manifest)?;
    let mut app = AppBuilder::at(&root).review_points(&points).build()?;

    app.request_review_point_manage();
    select_point(&mut app, point.id())?;
    manager_rename(&mut app);
    assert!(matches!(
        app.popup(),
        Some(Popup::ReviewPointRename(rename)) if rename.editor().text() == name
    ));
    key(&mut app, KeyCode::Enter);

    assert_eq!(
        app.review_points
            .as_ref()
            .and_then(|store| store.get(point.id()))
            .and_then(ReviewPoint::name),
        Some(name.as_str())
    );
    assert_eq!(fs::read(manifest)?, before);
    Ok(())
}

#[test]
fn undersized_rename_popup_ignores_edits_and_submit_but_keeps_escape_and_quit() -> anyhow::Result<()>
{
    let (dir, root) = fixture("review-point-rename-undersized")?;
    let points = dir.0.join("points");
    let mut workspace = Workspace::discover(&root)?;
    let mut store = ReviewPointStore::open(&points)?;
    let point = capture(&mut store, &mut workspace, &root, "before", "first\n")?;
    let mut app = AppBuilder::at(&root).review_points(&points).build()?;

    app.request_review_point_manage();
    select_point(&mut app, point.id())?;
    manager_rename(&mut app);
    let (width, height) = app.minimum_pane_size();
    app.resize(width.saturating_sub(1), height);
    assert!(!app.panes_fit());

    key(&mut app, KeyCode::Char('x'));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(
        app.popup(),
        Some(Popup::ReviewPointRename(rename)) if rename.editor().text() == "before"
    ));
    let reopened = ReviewPointStore::open(&points)?;
    assert_eq!(
        reopened.get(point.id()).and_then(ReviewPoint::name),
        Some("before")
    );

    key(&mut app, KeyCode::Esc);
    assert!(app.popup().is_none());

    app.resize(width, height);
    app.request_review_point_manage();
    select_point(&mut app, point.id())?;
    manager_rename(&mut app);
    app.resize(width.saturating_sub(1), height);
    key(&mut app, KeyCode::Char('q'));
    assert!(matches!(app.popup(), Some(Popup::ConfirmQuit)));
    key(&mut app, KeyCode::Esc);
    assert!(app.popup().is_none());
    Ok(())
}

#[test]
fn rename_errors_reload_and_reselect_the_stable_id() -> anyhow::Result<()> {
    let (dir, root) = fixture("review-point-rename-errors")?;
    let points = dir.0.join("points");
    let mut workspace = Workspace::discover(&root)?;
    let mut external = ReviewPointStore::open(&points)?;
    let first = capture(&mut external, &mut workspace, &root, "first", "first\n")?;
    let second = capture(&mut external, &mut workspace, &root, "second", "second\n")?;
    let mut app = AppBuilder::at(&root).review_points(&points).build()?;

    app.request_review_point_manage();
    select_point(&mut app, first.id())?;
    manager_rename(&mut app);
    if let Some(Popup::ReviewPointRename(rename)) = app.popup.as_mut() {
        rename.editor = fathomable_core::editor::Buffer::from_text("second");
    }
    key(&mut app, KeyCode::Enter);
    assert_eq!(selected_id(&app), Some(first.id()));
    assert!(
        app.message()
            .is_some_and(|message| message.contains("already in use"))
    );

    manager_rename(&mut app);
    if let Some(Popup::ReviewPointRename(rename)) = app.popup.as_mut() {
        rename.editor = fathomable_core::editor::Buffer::from_text("x".repeat(129));
    }
    key(&mut app, KeyCode::Enter);
    assert_eq!(selected_id(&app), Some(first.id()));
    assert!(
        app.message()
            .is_some_and(|message| message.contains("at most 128"))
    );

    manager_rename(&mut app);
    if let Some(Popup::ReviewPointRename(rename)) = app.popup.as_mut() {
        rename.editor = fathomable_core::editor::Buffer::from_text("bad\nname");
    }
    key(&mut app, KeyCode::Enter);
    assert_eq!(selected_id(&app), Some(first.id()));
    assert!(
        app.message()
            .is_some_and(|message| message.contains("must be one line"))
    );
    assert_eq!(
        app.review_points
            .as_ref()
            .and_then(|store| store.get(second.id()))
            .and_then(ReviewPoint::name),
        Some("second")
    );
    Ok(())
}

#[test]
fn comparison_picker_direct_rename_rejects_external_stale_snapshot() -> anyhow::Result<()> {
    let (dir, root) = fixture("review-point-rename-stale")?;
    let points = dir.0.join("points");
    let mut workspace = Workspace::discover(&root)?;
    let mut external = ReviewPointStore::open(&points)?;
    let point = capture(&mut external, &mut workspace, &root, "before", "first\n")?;
    let mut app = AppBuilder::at(&root).review_points(&points).build()?;

    app.open_picker(PickerKind::ComparisonReviewPoints);
    for character in "before".chars() {
        app.picker_char(character);
    }
    select_point(&mut app, point.id())?;
    ctrl_r(&mut app);
    external.rename(&point, Some("external"))?;
    if let Some(Popup::ReviewPointRename(rename)) = app.popup.as_mut() {
        rename.editor = fathomable_core::editor::Buffer::from_text("local");
    }
    key(&mut app, KeyCode::Enter);

    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.kind() == PickerKind::ComparisonReviewPoints
    ));
    assert_eq!(selected_id(&app), Some(point.id()));
    assert!(
        app.message()
            .is_some_and(|message| message.contains("changed elsewhere"))
    );
    assert_eq!(
        app.review_points
            .as_ref()
            .and_then(|store| store.get(point.id()))
            .and_then(ReviewPoint::name),
        Some("external")
    );
    Ok(())
}

#[test]
fn point_picker_hint_and_action_card_controls_are_clickable() -> anyhow::Result<()> {
    let (dir, root) = fixture("review-point-rename-mouse")?;
    let points = dir.0.join("points");
    let mut workspace = Workspace::discover(&root)?;
    let mut external = ReviewPointStore::open(&points)?;
    let point = capture(&mut external, &mut workspace, &root, "mouse", "first\n")?;
    let mut app = AppBuilder::at(&root).review_points(&points).build()?;

    app.open_picker(PickerKind::ComparisonReviewPoints);
    select_point(&mut app, point.id())?;
    let hint = match app.popup() {
        Some(Popup::Picker(picker)) => {
            let layout = draw::picker_layout(&app, picker);
            (0..app.pane_rows())
                .flat_map(|row| (0..app.size().0).map(move |column| (column, row)))
                .find(|(column, row)| layout.rename_at(*column, *row))
                .context("rename hint")?
        }
        _ => anyhow::bail!("comparison picker"),
    };
    click(&mut app, hint.0, hint.1);
    assert!(matches!(app.popup(), Some(Popup::ReviewPointRename(_))));
    key(&mut app, KeyCode::Esc);

    app.request_review_point_manage();
    select_point(&mut app, point.id())?;
    app.picker_confirm();
    let rename = draw::review_point_action_layout(&app);
    let rename_cell = (0..app.pane_rows())
        .flat_map(|row| (0..app.size().0).map(move |column| (column, row)))
        .find(|(column, row)| {
            rename.action_at(*column, *row) == Some(draw::ReviewPointActionHit::Rename)
        })
        .context("action-card rename")?;
    click(&mut app, rename_cell.0, rename_cell.1);
    assert!(matches!(app.popup(), Some(Popup::ReviewPointRename(_))));
    key(&mut app, KeyCode::Esc);

    let delete = draw::review_point_action_layout(&app);
    let delete_cell = (0..app.pane_rows())
        .flat_map(|row| (0..app.size().0).map(move |column| (column, row)))
        .find(|(column, row)| {
            delete.action_at(*column, *row) == Some(draw::ReviewPointActionHit::Delete)
        })
        .context("action-card delete")?;
    click(&mut app, delete_cell.0, delete_cell.1);
    assert!(matches!(
        app.popup(),
        Some(Popup::ConfirmReviewPointDelete { .. })
    ));
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.popup(), Some(Popup::ReviewPointAction(_))));

    let (width, height) = app.minimum_pane_size();
    app.resize(width, height);
    let layout = draw::review_point_action_layout(&app);
    assert!(layout.popup.width <= u16::try_from(width)?);
    assert!(layout.popup.height <= u16::try_from(app.pane_rows())?);
    let _ = crate::app::testing::buffer(&app)?;
    Ok(())
}

#[test]
fn manager_is_blocked_by_a_pending_annotation_draft() -> anyhow::Result<()> {
    let (dir, root) = fixture("review-point-manager-draft")?;
    let points = dir.0.join("points");
    let mut workspace = Workspace::discover(&root)?;
    let mut external = ReviewPointStore::open(&points)?;
    capture(&mut external, &mut workspace, &root, "point", "first\n")?;
    let threads = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = AppBuilder::at(&root)
        .review_points(&points)
        .options(move |mut options| {
            options.store = Some(threads);
            options
        })
        .build()?;

    app.open(std::path::Path::new("README.md"));
    app.start_new_comment();
    app.request_review_point_manage();

    assert!(matches!(app.popup(), Some(Popup::Compose(_))));
    assert!(
        app.message()
            .is_some_and(|message| message.contains("submit or cancel"))
    );
    Ok(())
}

#[test]
fn legacy_names_are_bounded_and_made_single_line_for_display() {
    let name = format!("line\n{}\u{2029}", "x".repeat(100));
    let rendered = super::review_point_name(Some(&name));

    assert!(!rendered.chars().any(char::is_control));
    assert!(!rendered.contains('\u{2029}'));
    assert!(rendered.chars().count() <= 65);
    assert!(rendered.ends_with('…'));
}
