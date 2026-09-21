//! Startup policy and bounded asynchronous discovery at the application boundary.

use std::fs;
use std::path::Path;

use fathomable_core::config::DiffMode;
use fathomable_core::workspace::Workspace;
use fathomable_testing::TempDir;

use super::{App, Options, PickerKind, Popup};

#[test]
fn fresh_plain_workspace_never_starts_an_implicit_content_comparison() -> anyhow::Result<()> {
    let dir = TempDir::new("plain-startup-policy")?;
    fs::write(dir.0.join("file"), b"content exceeds the comparison budget")?;
    let workspace = Workspace::discover(&dir.0)?;
    let mut options = Options::for_test(dir.0.clone());
    options.limits.comparison_bytes = 1;
    let app = App::new(workspace, 100, 30, options);
    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert_eq!(app.comparison.refresh_count(), 0);
    assert!(!app.background_pending());
    assert!(app.comparison().is_none());
    Ok(())
}

#[test]
fn partial_picker_remains_usable_and_names_its_limit() -> anyhow::Result<()> {
    let dir = TempDir::new("picker-limit")?;
    for i in 0..12 {
        fs::write(dir.0.join(format!("file{i}")), b"fixture")?;
    }
    let workspace = Workspace::discover(&dir.0)?;
    let mut options = Options::for_test(dir.0.clone());
    options.limits.discovery_entries = 3;
    let mut app = App::new(workspace, 100, 30, options);
    app.open_picker(PickerKind::Files);
    app.settle_background();
    let Some(Popup::Picker(picker)) = &app.popup else {
        anyhow::bail!("file picker was not opened");
    };
    assert_eq!(picker.total(), 3);
    assert!(
        picker
            .scope()
            .is_some_and(|scope| scope.contains("incomplete"))
    );
    Ok(())
}

#[test]
fn selecting_off_cancels_an_in_flight_enable_request() -> anyhow::Result<()> {
    let dir = TempDir::new("cancel-comparison-enable")?;
    fs::write(dir.0.join("file"), b"fixture")?;
    let workspace = Workspace::discover(&dir.0)?;
    let options = Options::for_test(dir.0.clone());
    let mut app = App::new(workspace, 100, 30, options);
    app.select_diff_mode(DiffMode::Normal);
    assert!(app.comparison.pending());
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert!(app.comparison.restore_mode.is_none());
    Ok(())
}

#[test]
fn comparison_exhaustion_never_enables_a_clean_comparison() -> anyhow::Result<()> {
    let dir = TempDir::new("app-comparison-limit")?;
    fs::write(dir.0.join("file"), b"fixture")?;
    let workspace = Workspace::discover(&dir.0)?;
    let mut options = Options::for_test(dir.0.clone());
    options.limits.comparison_bytes = 1;
    let mut app = App::new(workspace, 100, 30, options);
    app.select_diff_mode(DiffMode::Normal);
    app.settle_background();
    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert!(
        app.comparison
            .error()
            .is_some_and(|error| error.contains("limits.comparison-bytes"))
    );
    assert!(app.comparison().is_none());
    app.open(Path::new("file"));
    assert_eq!(app.current_path(), Path::new("file"));
    Ok(())
}

#[test]
fn watch_exhaustion_stays_visible_after_another_notice() -> anyhow::Result<()> {
    let dir = TempDir::new("persistent-watch-limit")?;
    let workspace = Workspace::discover(&dir.0)?;
    let mut app = App::new(workspace, 100, 30, Options::for_test(dir.0.clone()));
    app.set_watch_status(super::watch::WatchStatus::Limited {
        watched: 3,
        examined: 8,
        reason: super::watch::LimitReason::WorkspaceWatches,
    });
    app.notice("another interaction");
    assert!(app.comparison_badge().contains("watch limited"));
    assert!(app.status_lines().iter().any(|(label, value)| {
        label == "watching" && value.contains("workspace watch limit reached")
    }));
    Ok(())
}

#[test]
fn filesystem_events_preserve_a_pending_mode_activation() -> anyhow::Result<()> {
    for mode in [DiffMode::Normal, DiffMode::Unified] {
        let dir = TempDir::new(&format!("pending-mode-event-{mode:?}"))?;
        fs::write(dir.0.join("file"), "before\n")?;
        let workspace = Workspace::discover(&dir.0)?;
        let mut app = App::new(workspace, 100, 30, Options::for_test(dir.0.clone()));
        app.select_diff_mode(mode);
        fs::write(dir.0.join("file"), "after\n")?;
        app.on_events(vec![super::watch::Event::Change(dir.0.join("file"))]);
        assert_eq!(app.comparison.restore_mode, Some(mode));
        app.settle_background();
        assert_eq!(app.diff_mode(), mode);
        assert!(app.comparison().is_some());
    }
    Ok(())
}

#[test]
fn folding_a_truncated_flat_tree_does_not_erase_its_coverage_warning() -> anyhow::Result<()> {
    let dir = TempDir::new("flat-tree-warning")?;
    for i in 0..6 {
        fs::write(dir.0.join(format!("file{i}")), "fixture")?;
    }
    let workspace = Workspace::discover(&dir.0)?;
    let mut options = Options::for_test(dir.0.clone());
    options.limits.retained_paths = 3;
    let mut app = App::new(workspace, 100, 30, options);
    app.show_tree();
    assert!(app.comparison_badge().contains("files partial"));
    app.toggle_file_auto_unfold();
    app.settle_background();
    assert_eq!(app.tree().map(|tree| tree.rows().len()), Some(3));
    assert!(app.comparison_badge().contains("files partial"));
    assert!(
        app.tree_issue
            .as_deref()
            .is_some_and(|issue| issue.contains("budget"))
    );
    Ok(())
}

#[test]
fn filesystem_events_preserve_activation_with_an_immutable_target() -> anyhow::Result<()> {
    let dir = TempDir::new("pending-immutable-mode-event")?;
    fathomable_testing::git::init(&dir.0)?;
    fathomable_testing::git::commit_and_stage(&dir.0, &[("file", "committed\n")])?;
    let workspace = Workspace::discover(&dir.0)?;
    let endpoint = fathomable_core::workspace::ComparisonEndpoint::Commit(
        workspace.resolve_revision("HEAD")?.id(),
    );
    let mut app = App::new(workspace, 100, 30, Options::for_test(dir.0.clone()));
    app.select_diff_mode(DiffMode::Off);
    app.set_comparison_target(endpoint.clone());
    app.select_diff_mode(DiffMode::Unified);
    fs::write(dir.0.join("file"), "changed\n")?;
    app.on_events(vec![super::watch::Event::Change(dir.0.join("file"))]);
    assert_eq!(app.comparison.restore_mode, Some(DiffMode::Unified));
    app.settle_background();
    assert_eq!(app.diff_mode(), DiffMode::Unified);
    assert_eq!(app.comparison.target(), &endpoint);
    Ok(())
}
