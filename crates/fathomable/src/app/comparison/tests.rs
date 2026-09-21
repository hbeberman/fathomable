use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::Context as _;
use fathomable_core::XdgDirs;
use fathomable_core::annotations::{
    Author, ComparisonFacts, ContentIdentity, Draft, FullFileDigest, IndexFacts, IndexState,
    LineRange, OriginSide, OriginVersion, ReviewPointFacts, Store, WorkingTreeFacts,
    WorkingTreeState,
};
use fathomable_core::config::{DiffMode, HeadTransitionPolicy};
use fathomable_core::content::Content;
use fathomable_core::tree::Rule;
use fathomable_core::workspace::{CommitId, ComparisonEndpoint, Filter, Workspace};
use fathomable_testing::{TempDir, git};

use crate::app::input::bindings::Action;
use crate::app::testing::{AppBuilder, press, press_key};
use crate::app::{PickerKind, Popup};
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

#[test]
fn persistent_outputs_are_private_under_permissive_umasks() -> anyhow::Result<()> {
    for mask in ["000", "022"] {
        let output = std::process::Command::new("sh")
            .args([
                "-c",
                "umask \"$1\"; exec \"$2\" --exact app::comparison::tests::private_output_child --nocapture",
                "output-test",
                mask,
            ])
            .arg(std::env::current_exe()?)
            .env("FATHOMABLE_PRIVATE_OUTPUT_CHILD", mask)
            .output()?;
        assert!(
            output.status.success(),
            "umask {mask}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "the child must execute the permission assertions"
        );
    }
    Ok(())
}

#[test]
fn private_output_child() -> anyhow::Result<()> {
    use fathomable_core::{private_state, session::Id};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let Ok(mask) = std::env::var("FATHOMABLE_PRIVATE_OUTPUT_CHILD") else {
        return Ok(());
    };
    let fixture = TempDir::new(&format!("private-output-{mask}"))?;
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o755))?;
    let root = fixture.0.join("source");
    private_state::ensure_dir(&root)?;
    private_state::write(root.join("private.md"), "synthetic private source\n")?;
    let workspace = Workspace::discover(&root)?;
    let dirs = XdgDirs::resolve(|name| {
        (name == "XDG_STATE_HOME").then(|| fixture.0.join("state").into_os_string())
    });
    let report = crate::doctor::collect(&dirs, None, Some(&root), Some((90, 28)));
    assert!(
        report
            .lines()
            .iter()
            .any(|line| line.text == "log directory is writable")
    );
    let id = Id::mint();
    let _guard = crate::logging::init(&dirs, &id)?;
    tracing::warn!("synthetic private diagnostic");
    let crash = crate::logging::crash_path(&dirs, &id);
    crate::crash::arm(crash.clone(), crate::logging::log_path(&dirs, &id));
    crate::crash::observe(vec![(
        "document".to_owned(),
        "synthetic private source".to_owned(),
    )]);
    crate::crash::fatal(&anyhow::anyhow!("synthetic failure"));

    let mut comparison = super::State::load(&dirs, &workspace, super::Compare::default());
    comparison.toggle_whitespace();
    comparison.persist();
    let restored = super::State::load(&dirs, &workspace, super::Compare::default());
    assert_eq!(
        restored.compare().whitespace,
        comparison.compare().whitespace
    );
    for path in [
        crate::logging::log_path(&dirs, &id),
        crash.clone(),
        comparison.preference.clone(),
    ] {
        assert_eq!(
            fs::symlink_metadata(&path)?.mode() & 0o7777,
            0o600,
            "{}",
            path.display()
        );
        assert!(!fs::read(&path)?.is_empty(), "{}", path.display());
    }
    assert_eq!(fs::metadata(dirs.log_dir())?.mode() & 0o7777, 0o700);
    assert_eq!(
        fs::metadata(dirs.comparison_dir(&root))?.mode() & 0o7777,
        0o700
    );
    assert_eq!(fs::metadata(&fixture.0)?.mode() & 0o7777, 0o755);

    refuse_output_links(&mut comparison, &dirs, &root, &crash)
}

fn refuse_output_links(
    comparison: &mut super::State,
    dirs: &XdgDirs,
    root: &Path,
    crash: &Path,
) -> anyhow::Result<()> {
    use fathomable_core::{private_state, session::Id};
    use std::os::unix::fs::{MetadataExt, symlink};

    let unrelated = root.join("unrelated");
    private_state::write(&unrelated, "unchanged")?;
    fs::remove_file(&comparison.preference)?;
    symlink(&unrelated, &comparison.preference)?;
    comparison.persist();
    assert!(
        fs::symlink_metadata(&comparison.preference)?
            .file_type()
            .is_symlink()
    );
    fs::remove_file(&comparison.preference)?;
    let temporary = comparison
        .preference
        .with_extension(format!("{}.tmp", std::process::id()));
    symlink(&unrelated, &temporary)?;
    comparison.persist();
    assert!(fs::symlink_metadata(&temporary)?.file_type().is_symlink());
    assert!(!comparison.preference.exists());

    fs::remove_file(crash)?;
    symlink(&unrelated, crash)?;
    crate::crash::fatal(&anyhow::anyhow!("second synthetic failure"));
    assert!(fs::symlink_metadata(crash)?.file_type().is_symlink());
    let bad_id: Id = "1-1".parse()?;
    symlink(&unrelated, crate::logging::log_path(dirs, &bad_id))?;
    let Err(error) = crate::logging::init(dirs, &bad_id) else {
        return Err(anyhow::anyhow!("unsafe log path accepted"));
    };
    assert!(format!("{error:#}").contains("unsafe state path"));
    let probe = dirs
        .log_dir()
        .join(format!(".doctor-probe-{}", std::process::id()));
    symlink(&unrelated, &probe)?;
    let report = crate::doctor::collect(dirs, None, Some(root), Some((90, 28)));
    assert!(
        report
            .lines()
            .iter()
            .any(|line| line.text.contains("log directory is not writable"))
    );
    assert!(fs::symlink_metadata(&probe)?.file_type().is_symlink());
    assert_eq!(fs::read_to_string(&unrelated)?, "unchanged");
    assert_eq!(fs::metadata(&unrelated)?.mode() & 0o7777, 0o600);
    Ok(())
}

fn repository(name: &str) -> anyhow::Result<TempDir> {
    let dir = TempDir::new(name)?;
    fs::create_dir_all(dir.0.join("ws"))?;
    git::init(&dir.0.join("ws"))?;
    Ok(dir)
}

#[test]
fn one_pair_survives_file_switches_and_loads_historical_paths() -> anyhow::Result<()> {
    let dir = repository("comparison-global")?;
    let root = dir.0.join("ws");
    fs::write(root.join("gone.md"), "from A\n")?;
    fs::write(root.join("stay.md"), "same\n")?;
    git::commit_and_stage(&root, &[("gone.md", "from A\n"), ("stay.md", "same\n")])?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("first commit missing"))?;
    fs::remove_file(root.join("gone.md"))?;
    fs::write(root.join("new.md"), "from B\n")?;
    git::commit_and_stage(&root, &[("stay.md", "same\n"), ("new.md", "from B\n")])?;
    let second = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("second commit missing"))?;
    fs::remove_file(root.join("new.md"))?;

    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.settle_background();
    app.hunk_next();
    assert_eq!(app.current_path(), Path::new("gone.md"));
    app.hunk_next();
    assert_eq!(app.current_path(), Path::new("new.md"));
    app.show_tree();
    assert!(
        app.tree()
            .is_some_and(|tree| tree.contains(Path::new("new.md")))
    );
    assert_eq!(
        app.comparison_status()
            .get(Path::new("new.md"))
            .map(fathomable_core::status::Entry::state),
        Some(fathomable_core::status::State::Added)
    );
    app.open(Path::new("gone.md"));
    assert_eq!(app.view().text(), "from A\n");
    app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
    app.settle_background();
    assert!(app.view().diff_view());
    assert_eq!(
        app.comparison()
            .map(|comparison| comparison.base().to_string()),
        Some(CommitId::parse(&first)?.short().to_owned())
    );
    app.open(Path::new("new.md"));
    assert_eq!(app.view().text(), "from B\n");
    assert_eq!(app.comparison().map(|c| c.changes().len()), Some(2));
    Ok(())
}

#[test]
fn commit_to_working_opens_a_clean_deleted_path_from_the_selected_base() -> anyhow::Result<()> {
    let dir = repository("comparison-clean-deletion")?;
    let root = dir.0.join("ws");
    fs::write(root.join("gone.md"), "from A\n")?;
    git::commit_and_stage(&root, &[("gone.md", "from A\n")])?;
    let base = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("base commit missing"))?;
    fs::remove_file(root.join("gone.md"))?;
    git::commit_and_stage(&root, &[])?;

    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&base)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    app.settle_background();
    assert!(app.status().is_empty());
    app.open(Path::new("gone.md"));
    assert_eq!(app.view().text(), "from A\n");
    assert_eq!(
        app.banner(),
        Some("deleted in comparison · showing diff source")
    );
    Ok(())
}

#[test]
fn explicit_modes_use_the_space_d_bindings() -> anyhow::Result<()> {
    let dir = repository("comparison-bindings")?;
    let root = dir.0.join("ws");
    fs::write(root.join("a.md"), "one\n")?;
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.open(Path::new("a.md"));

    press(&mut app, " du");
    assert!(app.view().diff_view());
    press_key(&mut app, KeyCode::Esc);
    assert!(app.view().diff_view());
    press(&mut app, " dn");
    assert!(!app.view().diff_view());
    press(&mut app, " ds");
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.kind() == PickerKind::ComparisonBase
    ));
    press_key(&mut app, KeyCode::Esc);
    press(&mut app, " do");
    assert_eq!(app.diff_mode(), DiffMode::Off);
    press(&mut app, " dp");
    assert_eq!(
        app.review_points
            .as_ref()
            .map(fathomable_core::review_points::ReviewPointStore::len),
        None,
        "test app has no review-point store unless explicitly configured"
    );
    assert!(
        app.message()
            .is_some_and(|message| message.contains("review point"))
    );
    Ok(())
}

#[test]
fn head_parent_shortcut_selects_one_immutable_pair() -> anyhow::Result<()> {
    let dir = repository("comparison-head-parent")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .context("first commit")?;
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    let second = Workspace::discover(&root)?
        .head_commit()
        .context("second commit")?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    let refreshes = app.comparison.refresh_count();

    press(&mut app, " dl");

    assert_eq!(
        app.comparison.refresh_count(),
        refreshes.wrapping_add(1),
        "the endpoint pair is refreshed atomically"
    );
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&first)?)
    );
    assert_eq!(
        app.comparison.target(),
        &ComparisonEndpoint::Commit(CommitId::parse(&second)?)
    );
    assert_eq!(
        app.comparison_menu_pair(),
        ("HEAD~1".to_owned(), "HEAD".to_owned())
    );
    drop(app);

    let restored = AppBuilder::at(&root).unopened().build()?;
    assert_eq!(
        restored.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&first)?)
    );
    assert_eq!(
        restored.comparison.target(),
        &ComparisonEndpoint::Commit(CommitId::parse(&second)?)
    );
    assert_eq!(
        restored.comparison_menu_pair(),
        ("HEAD~1".to_owned(), "HEAD".to_owned())
    );
    Ok(())
}

#[test]
fn immutable_comparison_budget_covers_only_changed_blob_passes() -> anyhow::Result<()> {
    let dir = repository("comparison-immutable-budget")?;
    let root = dir.0.join("ws");
    let unchanged = "x".repeat(1_024);
    git::commit_and_stage(
        &root,
        &[("changed.txt", "old\n"), ("unchanged.txt", &unchanged)],
    )?;
    git::commit_and_stage(
        &root,
        &[("changed.txt", "new\n"), ("unchanged.txt", &unchanged)],
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(|mut options| {
            options.limits.comparison_bytes = 16;
            options
        })
        .build()?;

    press(&mut app, " dl");

    assert_eq!(app.comparison.error(), None);
    assert_eq!(
        app.comparison().map(fathomable_core::diff::Comparison::len),
        Some(1)
    );
    let changed = app
        .comparison_status()
        .get(Path::new("changed.txt"))
        .context("changed comparison entry")?;
    assert_eq!((changed.added(), changed.removed()), (1, 1));
    assert!(
        app.comparison_status()
            .get(Path::new("unchanged.txt"))
            .is_none()
    );
    Ok(())
}

#[test]
fn commit_parent_picker_selects_the_chosen_commit_and_first_parent() -> anyhow::Result<()> {
    let dir = repository("comparison-commit-parent")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .context("first commit")?;
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    let second = Workspace::discover(&root)?
        .head_commit()
        .context("second commit")?;
    git::commit_and_stage(&root, &[("a.md", "three\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    let refreshes = app.comparison.refresh_count();

    press(&mut app, " dc");
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.kind() == PickerKind::ComparisonCommit
    ));
    press(&mut app, &second[..8]);
    press_key(&mut app, KeyCode::Enter);

    assert_eq!(
        app.comparison.refresh_count(),
        refreshes.wrapping_add(1),
        "the endpoint pair is refreshed atomically"
    );
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&first)?)
    );
    assert_eq!(
        app.comparison.target(),
        &ComparisonEndpoint::Commit(CommitId::parse(&second)?)
    );
    Ok(())
}

#[test]
fn nested_tag_picker_retains_commit_parent_intent() -> anyhow::Result<()> {
    let dir = repository("comparison-tag-parent")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .context("first commit")?;
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    let tagged = Workspace::discover(&root)?
        .head_commit()
        .context("tagged commit")?;
    git::tag(&root, "review-base")?;
    git::commit_and_stage(&root, &[("a.md", "three\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;

    press(&mut app, " dc");
    press_key(&mut app, KeyCode::Down);
    press_key(&mut app, KeyCode::Enter);
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker))
            if picker.kind()
                == PickerKind::ComparisonTags(crate::app::ComparisonSide::CommitParent)
    ));
    press(&mut app, "review-base");
    press_key(&mut app, KeyCode::Enter);

    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&first)?)
    );
    assert_eq!(
        app.comparison.target(),
        &ComparisonEndpoint::Commit(CommitId::parse(&tagged)?)
    );
    Ok(())
}

#[test]
fn parentless_head_uses_empty_tree_commit_review() -> anyhow::Result<()> {
    let dir = repository("comparison-parentless")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    let refreshes = app.comparison.refresh_count();
    let head = Workspace::discover(&root)?.head_commit().context("head")?;

    press(&mut app, " dl");

    assert!(app.comparison.refresh_count() > refreshes);
    assert_eq!(
        (app.comparison.base(), app.comparison.target()),
        (
            &ComparisonEndpoint::EmptyTree,
            &ComparisonEndpoint::Commit(CommitId::parse(&head)?)
        )
    );
    Ok(())
}

#[test]
fn startup_mode_controls_presentation_and_off_falls_back_to_standard() -> anyhow::Result<()> {
    let dir = repository("comparison-startup-mode")?;
    let root = dir.0.join("ws");
    fs::write(root.join("README.md"), "old\n")?;
    git::commit_and_stage(&root, &[("README.md", "old\n")])?;
    fs::write(root.join("README.md"), "new\n")?;

    let unified = AppBuilder::at(&root)
        .options(|mut options| {
            options.diff.mode = DiffMode::Unified;
            options
        })
        .build()?;
    assert_eq!(unified.diff_mode(), DiffMode::Unified);
    assert!(unified.view().diff_view());

    let off = AppBuilder::at(&root)
        .options(|mut options| {
            options.diff.mode = DiffMode::Off;
            options
        })
        .build()?;
    assert_eq!(off.diff_mode(), DiffMode::Off);
    assert_eq!(off.last_active_diff_mode, DiffMode::Normal);
    assert_eq!(off.view().text(), "new\n");
    assert!(!off.view().diff_view());
    Ok(())
}

#[test]
fn source_display_is_retained_but_unavailable_while_unified() -> anyhow::Result<()> {
    let dir = repository("comparison-source-modes")?;
    let root = dir.0.join("ws");
    fs::write(root.join("README.md"), "# old\n")?;
    fs::write(root.join("main.rs"), "fn old() {}\n")?;
    git::commit_and_stage(
        &root,
        &[("README.md", "# old\n"), ("main.rs", "fn old() {}\n")],
    )?;
    fs::write(root.join("README.md"), "# new\n")?;
    fs::write(root.join("main.rs"), "fn new() {}\n")?;
    let mut app = AppBuilder::at(&root).build()?;

    press(&mut app, " vs");
    assert!(app.view().source_view());
    app.select_diff_mode(DiffMode::Unified);
    app.settle_background();
    assert!(app.view().diff_view());
    press(&mut app, " vs");
    assert_eq!(app.diff_mode(), DiffMode::Unified);
    assert!(app.view().diff_view());
    assert!(
        app.message()
            .is_some_and(|message| message.contains("unavailable in unified"))
    );
    app.select_diff_mode(DiffMode::Normal);
    app.settle_background();
    assert!(
        app.view().source_view(),
        "the source choice returns with the standard view"
    );
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    assert!(app.view().source_view());
    press(&mut app, " vs");
    assert!(!app.view().source_view(), "Off still permits rendered view");

    app.open(Path::new("main.rs"));
    assert!(app.view().source_view());
    press(&mut app, " vs");
    assert!(app.view().source_view(), "ordinary source cannot render");
    assert_eq!(
        app.message(),
        Some("rendered view is unavailable for this file")
    );
    app.select_diff_mode(DiffMode::Unified);
    app.settle_background();
    assert!(app.view().diff_view());
    app.open(Path::new("README.md"));
    assert!(
        app.view().diff_view(),
        "file switches keep unified presentation"
    );
    app.select_diff_mode(DiffMode::Normal);
    app.settle_background();
    assert!(
        !app.view().source_view(),
        "the eligible document restores its rendered choice"
    );
    app.open(Path::new("main.rs"));
    assert!(
        app.view().source_view(),
        "the ineligible document restores only source"
    );
    Ok(())
}

#[test]
fn off_clears_base_fallback_and_excludes_target_absent_paths() -> anyhow::Result<()> {
    let dir = repository("comparison-off-no-base-leak")?;
    let root = dir.0.join("ws");
    fs::write(root.join("gone.md"), "base secret\n")?;
    fs::write(root.join("stay.md"), "first\n")?;
    git::commit_and_stage(
        &root,
        &[("gone.md", "base secret\n"), ("stay.md", "first\n")],
    )?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("first commit"))?;
    fs::remove_file(root.join("gone.md"))?;
    fs::write(root.join("stay.md"), "second\n")?;
    git::commit_and_stage(&root, &[("stay.md", "second\n")])?;
    let second = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("second commit"))?;

    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.settle_background();
    app.open(Path::new("gone.md"));
    assert_eq!(app.view().text(), "base secret\n");
    app.select_diff_mode(DiffMode::Unified);
    app.settle_background();
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();

    assert_eq!(app.current_path(), Path::new("gone.md"));
    assert_eq!(app.view().text(), "");
    assert!(!app.view().diff_view());
    assert_eq!(
        app.current
            .and_then(|index| app.docs[index].comparison_notice.as_deref()),
        Some("not present in Target")
    );
    assert!(
        !app.comparison_snapshot_paths()
            .unwrap_or_default()
            .contains(&Path::new("gone.md").to_path_buf())
    );
    assert!(
        app.view()
            .layout()
            .lines()
            .iter()
            .all(|line| !line.text().contains("base secret"))
    );
    app.select_diff_mode(DiffMode::Normal);
    app.settle_background();
    assert_eq!(app.view().text(), "base secret\n");
    assert_eq!(
        app.banner(),
        Some("deleted in comparison · showing diff source")
    );
    Ok(())
}

#[test]
fn off_clears_file_content_when_target_enumeration_fails() -> anyhow::Result<()> {
    let dir = repository("comparison-off-target-enumeration-error")?;
    let root = dir.0.join("ws");
    fs::write(root.join("a.txt"), "must not survive\n")?;
    git::commit_and_stage(&root, &[("a.txt", "must not survive\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.open(Path::new("a.txt"));
    let unavailable =
        ComparisonEndpoint::Commit(CommitId::parse("0000000000000000000000000000000000000000")?);
    app.set_comparison_target(unavailable);
    app.settle_background();
    assert_eq!(app.view().text(), "must not survive\n");

    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert_eq!(app.view().text(), "");
    assert!(!app.view().diff_view());
    assert!(
        app.docs[app.current.context("current document")?]
            .comparison_notice
            .as_deref()
            .is_some_and(|notice| notice.contains("cannot read commit"))
    );
    Ok(())
}

#[test]
fn off_preserves_cursor_and_viewport_across_target_refreshes() -> anyhow::Result<()> {
    let dir = repository("comparison-off-refresh-position")?;
    let root = dir.0.join("ws");
    let mut text = String::new();
    for line in 1..=100 {
        writeln!(text, "line {line}")?;
    }
    fs::write(root.join("long.txt"), &text)?;
    git::commit_and_stage(&root, &[("long.txt", &text)])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.open(Path::new("long.txt"));
    app.view_mut().goto_source_line(70);
    app.view_mut().scroll_by(12);
    let line = app.view().cursor_source_line();
    let scroll = app.view().scroll();

    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    assert_eq!(app.view().cursor_source_line(), line);
    assert_eq!(app.view().scroll(), scroll);
    app.refresh_comparison();
    app.settle_background();
    assert_eq!(app.view().cursor_source_line(), line);
    assert_eq!(app.view().scroll(), scroll);
    Ok(())
}

#[test]
fn off_suspends_cached_gutters_and_counts_until_diff_returns() -> anyhow::Result<()> {
    let dir = repository("comparison-off-cached-diff")?;
    let root = dir.0.join("ws");
    fs::write(root.join("a.txt"), "one\ntwo\n")?;
    git::commit_and_stage(&root, &[("a.txt", "one\ntwo\n")])?;
    fs::write(root.join("a.txt"), "one\nchanged\n")?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.open(Path::new("a.txt"));
    assert!(
        app.view()
            .diff_counts()
            .is_some_and(|(added, removed)| { added > 0 && removed > 0 })
    );
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    assert_eq!(app.view().diff_counts(), None);

    app.select_diff_mode(DiffMode::Normal);
    app.settle_background();
    assert!(
        app.view()
            .diff_counts()
            .is_some_and(|(added, removed)| { added > 0 && removed > 0 })
    );
    Ok(())
}

#[test]
fn off_working_tree_filters_exclude_deleted_and_open_ignored_targets() -> anyhow::Result<()> {
    let dir = repository("comparison-off-working-filters")?;
    let root = dir.0.join("ws");
    fs::write(root.join(".gitignore"), "*.log\n")?;
    fs::write(root.join("README.md"), "tracked\n")?;
    git::commit_and_stage(
        &root,
        &[(".gitignore", "*.log\n"), ("README.md", "tracked\n")],
    )?;
    fs::remove_file(root.join("README.md"))?;
    fs::write(root.join("notes.txt"), "untracked\n")?;
    fs::write(root.join("build.log"), "ignored\n")?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.show_tree();
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();

    let contains = |app: &crate::app::App, path: &str| {
        app.tree()
            .is_some_and(|tree| tree.contains(Path::new(path)))
    };
    assert!(!contains(&app, "README.md"));
    assert!(
        !app.index(Filter::Visible)
            .iter()
            .any(|path| path == "README.md")
    );
    assert!(!contains(&app, "build.log"));
    assert!(contains(&app, "notes.txt"));

    app.files_toggle(Rule::Ignored);
    assert!(contains(&app, "build.log"));
    app.open(Path::new("build.log"));
    assert_eq!(app.view().text(), "ignored\n");
    app.files_toggle(Rule::Untracked);
    assert!(!contains(&app, "notes.txt"));
    assert!(contains(&app, "build.log"));
    Ok(())
}

#[test]
fn entering_off_reobserves_head_before_accepting_working_tree() -> anyhow::Result<()> {
    let dir = repository("comparison-off-reobserves-head")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.settle_background();

    git::commit_and_stage(&root, &[("a.txt", "two\n")])?;
    let current = Workspace::discover(&root)?
        .head_commit()
        .context("current HEAD")?;
    app.select_diff_mode(DiffMode::Off);

    assert_eq!(
        app.comparison
            .accepted_head()
            .and_then(|head| head.state().commit())
            .map(CommitId::as_str),
        Some(current.as_str())
    );
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(current)?)
    );
    Ok(())
}

#[test]
fn target_only_capture_rejects_head_movement_after_reading_paths() -> anyhow::Result<()> {
    let dir = repository("comparison-target-final-head-check")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let workspace = Workspace::discover(&root)?;
    let request = super::TargetRequest {
        root: root.clone(),
        endpoint: ComparisonEndpoint::Index,
        limits: workspace.limits().clone(),
        head: workspace.observe_head(7),
    };

    let result = super::target_paths_after_capture(
        request,
        fathomable_core::workspace::Cancellation::default(),
        || git::commit_and_stage(&root, &[("a.txt", "two\n")]).map_err(|error| error.to_string()),
    );

    assert!(
        result.is_err_and(|error| error.contains("HEAD changed during Target discovery")),
        "a target snapshot cannot retain a pre-capture HEAD"
    );
    Ok(())
}

#[test]
fn off_classifies_binary_and_over_limit_targets_before_text_conversion() -> anyhow::Result<()> {
    let dir = repository("comparison-off-content-policy")?;
    let root = dir.0.join("ws");
    fs::write(root.join("binary.bin"), [0, 0xff, 1, 2])?;
    let large = "x".repeat(1024 * 1024);
    git::commit_and_stage(&root, &[("large.txt", &large)])?;
    let commit = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("target commit"))?;
    fs::write(root.join("binary.bin"), [0, 0xff, 1, 2])?;

    let mut binary = AppBuilder::at(&root)
        .unopened()
        .options(|mut options| {
            options.diff.mode = DiffMode::Off;
            options
        })
        .build()?;
    binary.open(Path::new("binary.bin"));
    assert!(matches!(
        binary.docs[binary.current.context("binary document")?]
            .document
            .content(),
        Content::Binary { size: 4, .. }
    ));

    let mut limited = AppBuilder::at(&root)
        .unopened()
        .options(|mut options| {
            options.diff.mode = DiffMode::Off;
            options.viewer.max_file_size_mib = 0;
            options
        })
        .build()?;
    limited.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&commit)?));
    limited.open(Path::new("large.txt"));
    assert!(matches!(
        limited.docs[limited.current.context("large document")?]
            .document
            .content(),
        Content::TooLarge {
            size: 1_048_576,
            max_bytes: 0
        }
    ));
    Ok(())
}

#[test]
fn off_target_selection_ignores_base_and_base_selection_restores_mode() -> anyhow::Result<()> {
    let dir = repository("comparison-off-transitions")?;
    let root = dir.0.join("ws");
    fs::write(root.join("a.txt"), "one\n")?;
    git::commit_and_stage(&root, &[("a.txt", "one\n")])?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("first commit"))?;
    fs::write(root.join("a.txt"), "two\n")?;
    git::commit_and_stage(&root, &[("a.txt", "two\n")])?;
    let second = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("second commit"))?;

    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.open(Path::new("a.txt"));
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.settle_background();
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    app.set_comparison_base(ComparisonEndpoint::ReviewPoint("point".to_owned()));
    app.settle_background();
    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::ReviewPoint("point".to_owned())
    );
    assert!(
        app.message()
            .is_some_and(|message| message.contains("review points compare only"))
    );

    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert_eq!(app.view().text(), "one\n");
    assert!(app.comparison.error().is_none());
    app.select_diff_mode(DiffMode::Unified);
    app.settle_background();
    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert_eq!(
        app.comparison.target(),
        &ComparisonEndpoint::Commit(CommitId::parse(&first)?)
    );

    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    app.settle_background();
    fs::write(root.join("a.txt"), "working\n")?;
    let current = app
        .current
        .ok_or_else(|| anyhow::anyhow!("current document"))?;
    assert!(app.reload_doc(current).is_some());
    assert_eq!(app.view().text(), "working\n");

    let unavailable =
        ComparisonEndpoint::Commit(CommitId::parse("0000000000000000000000000000000000000000")?);
    app.set_comparison_base(unavailable.clone());
    app.settle_background();
    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert_eq!(app.comparison.base(), &unavailable);
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.settle_background();
    assert_eq!(app.diff_mode(), DiffMode::Normal);
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&second)?)
    );
    Ok(())
}

#[test]
fn review_point_picker_accepts_an_optional_name() -> anyhow::Result<()> {
    let dir = repository("comparison-point-name")?;
    let root = dir.0.join("ws");
    fs::write(root.join("a.md"), "one\n")?;
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .review_points(dir.0.join("points"))
        .build()?;
    app.set_comparison_target(ComparisonEndpoint::Index);
    app.settle_background();

    press(&mut app, " dpBefore fixes");
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.kind() == PickerKind::ReviewPointName
    ));
    press_key(&mut app, KeyCode::Enter);
    let points = app
        .review_points
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("review points"))?
        .list();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].name(), Some("Before fixes"));
    let id = points[0].id().to_owned();
    assert_eq!(app.comparison.base(), &ComparisonEndpoint::ReviewPoint(id));
    assert_eq!(app.comparison.target(), &ComparisonEndpoint::WorkingTree);
    assert_eq!(
        app.comparison().map(fathomable_core::diff::Comparison::len),
        Some(0)
    );
    Ok(())
}

#[test]
fn accepted_index_target_keeps_captured_bytes_after_live_index_changes() -> anyhow::Result<()> {
    let dir = repository("comparison-index-captured-document")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "committed\n")])?;
    git::stage(&root, &[("a.md", "captured\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::EmptyTree);
    app.set_comparison_target(ComparisonEndpoint::Index);
    app.settle_background();

    git::stage(&root, &[("a.md", "later\n")])?;
    app.open(Path::new("a.md"));
    assert_eq!(app.view().text(), "captured\n");
    let current = app
        .current
        .ok_or_else(|| anyhow::anyhow!("current document"))?;
    fs::write(root.join("a.md"), "newer working tree\n")?;
    let _ = app.reload_doc(current);
    app.apply_comparison_projection(current);
    assert_eq!(app.view().text(), "captured\n");
    assert_eq!(
        app.comparison_endpoint_text(&ComparisonEndpoint::Index, Path::new("a.md"))
            .map_err(anyhow::Error::msg)?,
        Some("captured\n".to_owned())
    );
    Ok(())
}

#[test]
fn off_mode_index_target_uses_its_accepted_manifest() -> anyhow::Result<()> {
    let dir = repository("comparison-off-index-captured-document")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "committed\n")])?;
    git::stage(&root, &[("a.md", "captured\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Index);
    app.settle_background();

    git::stage(&root, &[("a.md", "later\n")])?;
    app.open(Path::new("a.md"));
    assert_eq!(app.view().text(), "captured\n");
    Ok(())
}

#[test]
fn index_comparison_accepts_gitlink_missing_from_superproject_objects() -> anyhow::Result<()> {
    let dir = repository("comparison-index-missing-gitlink")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("module", "placeholder\n")])?;
    git::stage_gitlink(&root, "module", "1111111111111111111111111111111111111111")?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::EmptyTree);
    app.set_comparison_target(ComparisonEndpoint::Index);
    app.settle_background();

    assert!(app.comparison.error().is_none());
    assert_eq!(
        app.comparison_kind(Path::new("module")),
        Some(fathomable_core::diff::PathChangeKind::Unsupported)
    );
    Ok(())
}

#[test]
fn saving_a_point_restores_diff_mode_and_selects_working_tree() -> anyhow::Result<()> {
    let dir = repository("comparison-point-select-off")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .review_points(dir.0.join("points"))
        .build()?;
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();

    app.save_review_point(None);
    app.settle_background();

    assert_eq!(app.diff_mode(), DiffMode::Normal);
    assert!(matches!(
        app.comparison.base(),
        ComparisonEndpoint::ReviewPoint(_)
    ));
    assert_eq!(app.comparison.target(), &ComparisonEndpoint::WorkingTree);
    Ok(())
}

#[test]
fn failed_preference_write_keeps_the_new_point_selected_for_this_viewer() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;

    let dir = repository("comparison-point-preference-failure")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    fs::write(root.join("a.md"), "point\n")?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .review_points(dir.0.join("points"))
        .build()?;
    let preference = app.comparison.preference.clone();
    let original = fs::read(&preference)?;
    let unrelated = root.join("unrelated");
    fs::write(&unrelated, "unchanged")?;
    let temporary = preference.with_extension(format!("{}.tmp", std::process::id()));
    symlink(&unrelated, &temporary)?;

    app.save_review_point(Some("local"));
    app.settle_background();

    let point = app
        .review_points
        .as_ref()
        .and_then(|store| store.list().first().cloned())
        .context("saved point")?;
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::ReviewPoint(point.id().to_owned())
    );
    assert_eq!(app.comparison.target(), &ComparisonEndpoint::WorkingTree);
    assert_eq!(fs::read(preference)?, original);
    assert!(
        app.message()
            .is_some_and(|message| message.contains("selected for this viewer")
                && message.contains("preference was not saved"))
    );
    Ok(())
}

#[test]
fn review_point_picker_entry_compares_against_working() -> anyhow::Result<()> {
    let dir = repository("comparison-point-endpoint")?;
    let root = dir.0.join("ws");
    fs::write(root.join("a.md"), "one\n")?;
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    fs::write(root.join("a.md"), "point\n")?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .review_points(dir.0.join("points"))
        .build()?;
    app.save_review_point(Some("P"));
    app.settle_background();
    let point = app
        .review_points
        .as_ref()
        .and_then(|store| store.list().first().cloned())
        .ok_or_else(|| anyhow::anyhow!("saved point"))?;
    fs::write(root.join("a.md"), "working\n")?;
    app.set_comparison_base(ComparisonEndpoint::ReviewPoint(point.id().to_owned()));
    app.settle_background();

    assert_eq!(
        app.comparison()
            .map(fathomable_core::diff::Comparison::base),
        Some(&ComparisonEndpoint::ReviewPoint(point.id().to_owned()))
    );
    assert_eq!(
        app.comparison().map(fathomable_core::diff::Comparison::len),
        Some(1)
    );
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one fixture checks point identity, checkout ownership, landing, and deletion"
)]
fn review_point_membership_is_exact_then_survives_landing_and_deletion() -> anyhow::Result<()> {
    let dir = repository("comparison-point-membership")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .review_points(dir.0.join("points"))
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.save_review_point(Some("P"));
    app.settle_background();
    let point = app
        .review_points
        .as_ref()
        .and_then(|points| points.list().first().cloned())
        .context("point P")?;
    let foreign_root = dir.0.join("foreign");
    git::init(&foreign_root)?;
    git::commit_and_stage(&foreign_root, &[("a.md", "one\n")])?;
    let foreign_checkout = Workspace::discover(&foreign_root)?.identity();
    let id = app.store.as_mut().context("thread store")?.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "point thread",
        )
        .at_review_point(
            ReviewPointFacts::new(
                point.id(),
                point.head().map(ToString::to_string),
                Some(ContentIdentity::from_text("one\n")),
                point.checkout_identity(),
                FullFileDigest::from_bytes(b"one\n"),
            ),
            OriginSide::Base,
        ),
        "one\n",
        1,
    )?;
    let foreign = app.store.as_mut().context("thread store")?.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "foreign point thread",
        )
        .at_review_point(
            ReviewPointFacts::new(
                point.id(),
                point.head().map(ToString::to_string),
                Some(ContentIdentity::from_text("one\n")),
                foreign_checkout,
                FullFileDigest::from_bytes(b"one\n"),
            ),
            OriginSide::Base,
        ),
        "one\n",
        2,
    )?;
    app.refresh_after_thread_store_change();
    app.open(Path::new("a.md"));
    assert!(app.marks().iter().any(|mark| mark.id() == &id));
    assert!(
        app.marks().iter().all(|mark| mark.id() != &foreign),
        "a point origin from another checkout does not match"
    );

    app.save_review_point(Some("Q"));
    app.settle_background();
    assert!(
        app.marks().iter().all(|mark| mark.id() != &id),
        "a distinct point with the same baseline is not the origin point"
    );
    app.set_comparison_base(ComparisonEndpoint::ReviewPoint(point.id().to_owned()));
    app.settle_background();
    assert!(app.marks().iter().any(|mark| mark.id() == &id));

    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    app.refresh_comparison();
    app.settle_background();
    let landed = app
        .thread(&id)
        .and_then(fathomable_core::annotations::Thread::landed_commit)
        .map(str::to_owned)
        .context("landed commit")?;
    assert!(app.marks().iter().any(|mark| mark.id() == &id));

    app.review_points
        .as_mut()
        .context("review points")?
        .delete(point.id())?;
    app.refresh_comparison();
    app.settle_background();
    assert_eq!(
        app.thread(&id)
            .and_then(fathomable_core::annotations::Thread::landed_commit),
        Some(landed.as_str())
    );
    assert!(
        app.marks().iter().all(|mark| mark.id() != &id),
        "deleting the point preserves its original association"
    );
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&landed)?));
    app.settle_background();
    assert!(
        app.marks().iter().all(|mark| mark.id() != &id),
        "Base-side evidence is not Target content even after landing"
    );
    Ok(())
}

#[test]
fn filtered_revision_picker_uses_the_highlighted_choice() -> anyhow::Result<()> {
    let dir = repository("comparison-filtered-revision")?;
    let root = dir.0.join("ws");
    fs::write(root.join("a.md"), "one\n")?;
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let feature = dir.0.join("feature");
    git::worktree_add(&root, &feature, "feature")?;
    let feature_head = Workspace::discover(&feature)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("feature head"))?;
    fs::write(root.join("a.md"), "two\n")?;
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;

    app.open_picker(PickerKind::ComparisonBase);
    app.settle_background();
    press(&mut app, "feature");
    press_key(&mut app, KeyCode::Enter);

    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(feature_head)?)
    );
    Ok(())
}

#[test]
fn immutable_comparison_synthesizes_modified_paths_missing_from_checkout() -> anyhow::Result<()> {
    let dir = repository("comparison-virtual-modified")?;
    let root = dir.0.join("ws");
    fs::write(root.join("a.md"), "one\n")?;
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("first"))?;
    fs::write(root.join("a.md"), "two\n")?;
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    let second = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("second"))?;
    fs::remove_file(root.join("a.md"))?;
    git::commit_and_stage(&root, &[])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(second)?));
    app.settle_background();
    app.show_tree();

    assert!(
        app.tree()
            .is_some_and(|tree| tree.contains(Path::new("a.md")))
    );
    assert_eq!(
        app.comparison_status()
            .get(Path::new("a.md"))
            .map(fathomable_core::status::Entry::state),
        Some(fathomable_core::status::State::Modified)
    );
    Ok(())
}

#[test]
fn historical_target_confines_the_tree_picker_and_file_view() -> anyhow::Result<()> {
    let dir = repository("comparison-historical-tree")?;
    let root = dir.0.join("ws");
    fs::write(root.join("base-only.md"), "base\n")?;
    fs::write(root.join("shared.md"), "first\n")?;
    fs::write(root.join("unchanged.md"), "same\n")?;
    git::commit_and_stage(
        &root,
        &[
            ("base-only.md", "base\n"),
            ("shared.md", "first\n"),
            ("unchanged.md", "same\n"),
        ],
    )?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("first"))?;
    fs::remove_file(root.join("base-only.md"))?;
    fs::write(root.join("shared.md"), "second\n")?;
    fs::write(root.join("target-only.md"), "target\n")?;
    git::commit_and_stage(
        &root,
        &[
            ("shared.md", "second\n"),
            ("target-only.md", "target\n"),
            ("unchanged.md", "same\n"),
        ],
    )?;
    let second = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("second"))?;
    fs::write(root.join("current-only.md"), "current\n")?;
    fs::write(root.join("shared.md"), "third\n")?;
    git::commit_and_stage(
        &root,
        &[
            ("current-only.md", "current\n"),
            ("shared.md", "third\n"),
            ("target-only.md", "target\n"),
            ("unchanged.md", "same\n"),
        ],
    )?;

    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(second)?));
    app.settle_background();
    app.show_tree();

    let tree_paths: Vec<_> = app
        .tree()
        .into_iter()
        .flat_map(fathomable_core::tree::Tree::rows)
        .map(|row| row.path().to_string_lossy().into_owned())
        .collect();
    let expected = [
        "base-only.md",
        "shared.md",
        "target-only.md",
        "unchanged.md",
    ];
    assert_eq!(tree_paths, expected);
    assert_eq!(app.index(Filter::Visible), expected);
    app.open(Path::new("shared.md"));
    assert_eq!(app.view().text(), "second\n");

    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    app.settle_background();
    assert!(
        app.tree()
            .is_some_and(|tree| tree.contains(Path::new("current-only.md")))
    );
    Ok(())
}

#[test]
fn untouched_default_base_follows_head_after_restart() -> anyhow::Result<()> {
    let dir = repository("comparison-default-persistence")?;
    let root = dir.0.join("ws");
    fs::write(root.join("a.md"), "one\n")?;
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("first"))?;
    let state = dir.0.join("state").into_os_string();
    let dirs = XdgDirs::resolve(|name| (name == "XDG_STATE_HOME").then(|| state.clone()));
    let first_dirs = dirs.clone();
    let app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.dirs = first_dirs;
            options
        })
        .build()?;
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&first)?)
    );
    assert_eq!(
        app.comparison_menu_pair(),
        ("HEAD".to_owned(), "WorkingTree".to_owned())
    );
    drop(app);
    let preference = fs::read_to_string(dirs.comparison_dir(&root).join("comparison.json"))?;
    assert!(preference.contains("\"version\": 3"));
    assert!(preference.contains("\"intent\": \"follow-head\""));
    assert!(!preference.contains("\"base\":"));

    fs::write(root.join("a.md"), "two\n")?;
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    let second = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("second"))?;
    let app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.dirs = dirs;
            options
        })
        .build()?;
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&second)?)
    );
    assert_eq!(app.comparison_menu_pair().0, "HEAD");
    Ok(())
}

#[test]
fn ask_pin_advances_then_can_pin_future_head_movement() -> anyhow::Result<()> {
    let dir = repository("comparison-ask-pin")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.settle_background();

    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    let second = Workspace::discover(&root)?
        .head_commit()
        .context("second commit")?;
    app.refresh_comparison();
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&second)?)
    );
    assert_eq!(
        app.comparison.source_intent(),
        &super::SourceIntent::FollowHead
    );
    assert!(app.head_transition_prompt.is_some());
    app.resolve_head_transition_prompt(false);
    assert_eq!(
        app.comparison.source_intent(),
        &super::SourceIntent::Pinned(ComparisonEndpoint::Commit(CommitId::parse(&second)?))
    );
    assert_eq!(app.comparison.base_alias(), None);

    git::commit_and_stage(&root, &[("a.md", "three\n")])?;
    app.refresh_comparison();
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&second)?)
    );
    assert!(app.head_transition_prompt.is_none());
    Ok(())
}

#[test]
fn ask_follow_advances_pinned_by_default() -> anyhow::Result<()> {
    let dir = repository("comparison-ask-follow")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(|mut options| {
            options.diff.head_transition = HeadTransitionPolicy::AskFollow;
            options
        })
        .build()?;
    app.settle_background();

    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    let second = Workspace::discover(&root)?
        .head_commit()
        .context("second commit")?;
    app.refresh_comparison();
    assert_eq!(
        app.comparison.source_intent(),
        &super::SourceIntent::Pinned(ComparisonEndpoint::Commit(CommitId::parse(&second)?))
    );
    assert!(app.head_transition_prompt.is_some());
    app.resolve_head_transition_prompt(true);
    assert_eq!(
        app.comparison.source_intent(),
        &super::SourceIntent::FollowHead
    );
    assert_eq!(
        app.comparison.base_alias(),
        Some(&super::EndpointAlias::Head)
    );
    Ok(())
}

#[test]
fn transition_picker_cannot_confirm_or_dismiss_a_superseded_prompt() -> anyhow::Result<()> {
    let dir = repository("comparison-transition-picker-token")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.settle_background();

    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    app.refresh_comparison();
    app.open_picker(PickerKind::HeadTransition);
    app.picker_move(1);
    git::commit_and_stage(&root, &[("a.md", "three\n")])?;
    let third = Workspace::discover(&root)?
        .head_commit()
        .context("third commit")?;
    app.refresh_comparison();
    app.picker_confirm();
    assert_eq!(
        app.comparison.source_intent(),
        &super::SourceIntent::FollowHead
    );
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&third)?)
    );
    assert!(
        app.head_transition_prompt.is_some(),
        "confirming the old card must preserve the current prompt"
    );

    app.open_picker(PickerKind::HeadTransition);
    git::commit_and_stage(&root, &[("a.md", "four\n")])?;
    app.refresh_comparison();
    app.picker_escape();
    assert!(
        app.head_transition_prompt.is_some(),
        "dismissing the old card must preserve the current prompt"
    );
    Ok(())
}

#[test]
fn transition_choice_reobserves_head_before_watcher_delivery() -> anyhow::Result<()> {
    let dir = repository("comparison-transition-picker-reobserve")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.settle_background();

    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    app.refresh_comparison();
    app.open_picker(PickerKind::HeadTransition);
    app.picker_move(1);
    git::commit_and_stage(&root, &[("a.md", "three\n")])?;
    let third = Workspace::discover(&root)?
        .head_commit()
        .context("third commit")?;
    app.picker_confirm();

    assert_eq!(
        app.comparison.source_intent(),
        &super::SourceIntent::FollowHead
    );
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&third)?)
    );
    assert!(
        app.head_transition_prompt.is_some(),
        "the newly observed transition remains available"
    );
    Ok(())
}

#[test]
fn persistent_transition_prompt_works_without_toasts_and_dismisses() -> anyhow::Result<()> {
    let dir = repository("comparison-persistent-prompt")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(|mut options| {
            options.watch.toast = std::time::Duration::ZERO;
            options
        })
        .build()?;
    app.settle_background();
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    app.refresh_comparison();
    assert!(app.toasts().is_empty());
    assert!(
        app.transition_prompt_text()
            .is_some_and(|text| text.contains("HEAD moved"))
    );

    let pane = ratatui::layout::Rect::new(
        u16::try_from(app.sidebar_width())?,
        u16::try_from(app.pane_top())?,
        u16::try_from(app.size().0.saturating_sub(app.sidebar_width()))?,
        u16::try_from(app.pane_rows())?,
    );
    let area = crate::app::draw::transition_prompt_area(&app, pane).context("prompt area")?;
    crate::app::input::mouse::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: area.x,
            row: area.y,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert!(matches!(
        app.popup(),
        Some(Popup::Picker(picker)) if picker.kind() == PickerKind::HeadTransition
    ));
    app.picker_escape();
    assert!(!app.head_transition_pending());
    Ok(())
}

#[test]
fn accepted_index_equal_to_new_head_offers_pin_or_keep() -> anyhow::Result<()> {
    let dir = repository("comparison-index-transition")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::Index);
    app.settle_background();

    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let new_head = Workspace::discover(&root)?
        .head_commit()
        .context("new head")?;
    app.refresh_comparison();
    assert!(app.comparison.pending());
    app.settle_background();
    assert!(
        app.index_transition_prompt.is_some(),
        "accepted post-transition refresh must not discard immutable pre-transition evidence"
    );
    app.resolve_index_transition_prompt(true);
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(new_head)?)
    );
    Ok(())
}

#[test]
fn automatic_head_policies_do_not_prompt() -> anyhow::Result<()> {
    for (name, policy, follows) in [
        (
            "comparison-follow-policy",
            HeadTransitionPolicy::Follow,
            true,
        ),
        ("comparison-pin-policy", HeadTransitionPolicy::Pin, false),
    ] {
        let dir = repository(name)?;
        let root = dir.0.join("ws");
        git::commit_and_stage(&root, &[("a.md", "one\n")])?;
        let mut app = AppBuilder::at(&root)
            .unopened()
            .options(move |mut options| {
                options.diff.head_transition = policy;
                options
            })
            .build()?;
        app.settle_background();
        git::commit_and_stage(&root, &[("a.md", "two\n")])?;
        app.refresh_comparison();
        assert!(app.head_transition_prompt.is_none());
        assert_eq!(
            app.comparison.source_intent() == &super::SourceIntent::FollowHead,
            follows
        );
        assert_eq!(
            app.comparison.base_alias() == Some(&super::EndpointAlias::Head),
            follows,
            "only a following policy retains the presentation alias"
        );
    }
    Ok(())
}

#[test]
fn endpoint_selection_invalidates_a_pending_head_prompt() -> anyhow::Result<()> {
    let dir = repository("comparison-stale-transition")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.settle_background();
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    app.refresh_comparison();
    let stale = app
        .head_transition_prompt
        .clone()
        .context("pending prompt")?;
    app.set_comparison_base(ComparisonEndpoint::EmptyTree);
    assert!(app.head_transition_prompt.is_none());
    assert!(!app.comparison.resolve_head_prompt(&stale, false));
    Ok(())
}

#[test]
fn normal_membership_uses_exact_source_side_and_target_commit() -> anyhow::Result<()> {
    let dir = repository("comparison-origin-membership")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let base = Workspace::discover(&root)?.head_commit().context("base")?;
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    let target = Workspace::discover(&root)?
        .head_commit()
        .context("target")?;
    let path = dir.0.join("threads.jsonl");
    let mut store = Store::open(&path)?;
    let draft = |body: &str, version: OriginVersion, side: OriginSide| {
        Draft::new(
            Author::agent("reviewer"),
            Path::new("a.md"),
            LineRange::new(1, 1),
            body,
        )
        .at_source(version, side)
    };
    let source = store.annotate(
        draft("source", OriginVersion::commit(&base), OriginSide::Base),
        "one\n",
        1,
    )?;
    let target_thread = store.annotate(
        draft("target", OriginVersion::commit(&target), OriginSide::Target),
        "two\n",
        2,
    )?;
    store.annotate(
        draft(
            "wrong side",
            OriginVersion::commit(&base),
            OriginSide::Target,
        ),
        "one\n",
        3,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&base)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.settle_background();
    app.open(Path::new("a.md"));
    let ids = app
        .marks()
        .iter()
        .map(|mark| mark.id().clone())
        .collect::<Vec<_>>();
    assert_eq!(ids, [source, target_thread]);
    Ok(())
}

#[test]
fn commit_content_does_not_alias_the_mutable_head() -> anyhow::Result<()> {
    let dir = repository("comparison-accepted-membership")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let first = Workspace::discover(&root)?.head_commit().context("first")?;
    let checkout = Workspace::discover(&root)?.identity();
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let id = store.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("a.md"),
            LineRange::new(1, 1),
            "first commit",
        )
        .at_source(OriginVersion::commit(&first), OriginSide::Unspecified),
        "one\n",
        1,
    )?;
    let reviewed = store.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("a.md"),
            LineRange::new(1, 1),
            "review",
        )
        .at_selected_commit(CommitId::parse(&first)?),
        "one\n",
        2,
    )?;
    let working = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "working",
        )
        .with_working_tree_facts(WorkingTreeFacts::new(
            Some(first.clone()),
            WorkingTreeState::Clean,
            Some(ContentIdentity::from_text("one\n")),
            checkout,
            FullFileDigest::from_bytes(b"one\n"),
        )),
        "one\n",
        3,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.open(Path::new("a.md"));
    app.settle_background();
    assert!(
        app.marks().iter().all(|mark| mark.id() != &id),
        "WorkingTree is not an alias for its accepted HEAD"
    );
    assert!(!app.normal_thread(app.thread(&reviewed).context("review")?));
    assert!(app.normal_thread(app.thread(&working).context("working")?));
    fs::write(root.join("a.md"), "dirty\n")?;
    app.refresh_comparison();
    app.settle_background();
    assert!(
        app.marks().iter().all(|mark| mark.id() != &id),
        "dirty WorkingTree still does not alias HEAD"
    );
    assert!(!app.normal_thread(app.thread(&reviewed).context("dirty review")?));
    assert!(app.normal_thread(app.thread(&working).context("dirty working")?));
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    app.refresh_comparison();
    assert!(
        app.marks().iter().all(|mark| mark.id() != &id),
        "pending mutable presentation still does not alias HEAD"
    );
    app.settle_background();
    assert!(
        app.marks().iter().all(|mark| mark.id() != &id),
        "the accepted WorkingTree context follows its captured HEAD"
    );
    Ok(())
}

#[test]
fn commit_target_membership_does_not_follow_commit_as_source() -> anyhow::Result<()> {
    let dir = repository("comparison-commit-target-membership")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "parent\n")])?;
    let parent = Workspace::discover(&root)?
        .head_commit()
        .context("parent")?;
    git::commit_and_stage(&root, &[("a.md", "target\n")])?;
    let target = Workspace::discover(&root)?
        .head_commit()
        .context("target")?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let id = store.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("a.md"),
            LineRange::new(1, 1),
            "target commit",
        )
        .at_selected_commit(CommitId::parse(&target)?),
        "target\n",
        1,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;

    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&parent)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.settle_background();
    assert!(app.normal_thread(app.thread(&id).context("commit target")?));

    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    app.settle_background();
    assert!(!app.normal_thread(app.thread(&id).context("working target")?));

    app.set_comparison_target(ComparisonEndpoint::Index);
    app.settle_background();
    assert!(!app.normal_thread(app.thread(&id).context("index target")?));
    Ok(())
}

#[test]
fn clean_working_tree_landing_is_evidence_not_membership() -> anyhow::Result<()> {
    let dir = repository("comparison-clean-working-landing")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "parent\n")])?;
    let parent = Workspace::discover(&root)?
        .head_commit()
        .context("parent")?;
    git::commit_and_stage(&root, &[("a.md", "head\n")])?;
    let workspace = Workspace::discover(&root)?;
    let head = workspace.head_commit().context("head")?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let id = store.annotate(
        Draft::new(
            Author::agent("unselected"),
            Path::new("a.md"),
            LineRange::new(1, 1),
            "working-tree origin",
        )
        .with_working_tree_facts(WorkingTreeFacts::new(
            Some(head.clone()),
            WorkingTreeState::Clean,
            Some(ContentIdentity::from_text("head\n")),
            workspace.identity(),
            FullFileDigest::from_bytes(b"head\n"),
        )),
        "head\n",
        1,
    )?;
    let candidate = store
        .thread(&id)
        .and_then(fathomable_core::annotations::Thread::landing_candidate)
        .context("landing candidate")?;
    assert_eq!(
        store.land(&candidate, &CommitId::parse(&head)?)?,
        fathomable_core::annotations::LandingOutcome::Applied
    );
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.settle_background();
    assert!(app.normal_thread(app.thread(&id).context("working tree")?));

    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&parent)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&head)?));
    app.settle_background();
    assert_eq!(
        app.thread(&id).and_then(|thread| thread.landed_commit()),
        Some(head.as_str())
    );
    assert!(!app.normal_thread(app.thread(&id).context("landed thread")?));

    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&head)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    app.settle_background();
    assert!(app.normal_thread(app.thread(&id).context("working target")?));
    Ok(())
}

#[test]
fn base_membership_requires_the_recorded_ordered_comparison() -> anyhow::Result<()> {
    let dir = repository("comparison-base-ordered-pair")?;
    let root = dir.0.join("ws");
    fs::write(root.join("gone.md"), "present\n")?;
    git::commit_and_stage(&root, &[("gone.md", "present\n")])?;
    let source = Workspace::discover(&root)?
        .head_commit()
        .context("source")?;
    fs::remove_file(root.join("gone.md"))?;
    git::commit_and_stage(&root, &[])?;
    let target = Workspace::discover(&root)?
        .head_commit()
        .context("target")?;
    fs::write(root.join("other.md"), "later\n")?;
    git::commit_and_stage(&root, &[("other.md", "later\n")])?;
    let later = Workspace::discover(&root)?.head_commit().context("later")?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("gone.md"),
            LineRange::new(1, 1),
            "deletion",
        )
        .at_source(OriginVersion::commit(&source), OriginSide::Base)
        .in_comparison(ComparisonFacts::new(
            OriginVersion::commit(&source),
            OriginVersion::commit(&target),
        )),
        "present\n",
        1,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;

    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&source)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.settle_background();
    assert!(app.normal_thread(app.thread(&id).context("recorded pair")?));

    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&later)?));
    app.settle_background();
    assert!(!app.normal_thread(app.thread(&id).context("different target")?));
    Ok(())
}

#[derive(Clone, Copy)]
enum MutableTarget {
    WorkingTree,
    Index,
}

fn linked_worktree_rejects_foreign_base_deletion(
    name: &str,
    target: MutableTarget,
) -> anyhow::Result<()> {
    let dir = TempDir::new(name)?;
    let main = dir.0.join("main");
    fs::create_dir_all(&main)?;
    git::init(&main)?;
    fs::write(main.join("gone.md"), "present\n")?;
    git::commit_and_stage(&main, &[("gone.md", "present\n")])?;
    let head = Workspace::discover(&main)?
        .head_commit()
        .context("shared HEAD")?;
    let linked = dir.0.join("linked");
    git::worktree_add(&main, &linked, "linked")?;
    match target {
        MutableTarget::WorkingTree => fs::remove_file(main.join("gone.md"))?,
        MutableTarget::Index => git::stage(&main, &[])?,
    }
    let endpoint = match target {
        MutableTarget::WorkingTree => ComparisonEndpoint::WorkingTree,
        MutableTarget::Index => ComparisonEndpoint::Index,
    };
    let store_path = dir.0.join("threads.jsonl");
    let store = Store::open(&store_path)?;
    let mut source_app = AppBuilder::at(&main)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    source_app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&head)?));
    source_app.settle_background();
    source_app.set_comparison_target(endpoint.clone());
    source_app.settle_background();
    source_app.open(Path::new("gone.md"));
    source_app.start_new_comment();
    press(&mut source_app, "checkout-local deletion");
    source_app.compose_submit();
    let id = source_app
        .store
        .as_ref()
        .and_then(|store| store.threads().first())
        .context("base-side deletion thread")?
        .id()
        .clone();
    let thread = source_app.thread(&id).context("source thread")?;
    assert_eq!(thread.origin_side(), OriginSide::Base);
    assert_eq!(
        thread.comparison().and_then(ComparisonFacts::checkout),
        Some(&Workspace::discover(&main)?.identity())
    );
    assert!(source_app.normal_thread(thread));
    drop(source_app);

    let store = Store::open(&store_path)?;
    let mut linked_app = AppBuilder::at(&linked)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    linked_app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&head)?));
    linked_app.settle_background();
    linked_app.set_comparison_target(endpoint);
    linked_app.settle_background();
    assert!(
        !linked_app.normal_thread(linked_app.thread(&id).context("linked thread")?),
        "equal mutable endpoint and observed HEAD must not cross checkout identity"
    );
    Ok(())
}

#[test]
fn mutable_base_deletions_do_not_cross_linked_worktrees() -> anyhow::Result<()> {
    linked_worktree_rejects_foreign_base_deletion(
        "comparison-working-linked-checkout",
        MutableTarget::WorkingTree,
    )?;
    linked_worktree_rejects_foreign_base_deletion(
        "comparison-index-linked-checkout",
        MutableTarget::Index,
    )
}

#[test]
fn mutable_endpoint_membership_requires_checkout_identity() -> anyhow::Result<()> {
    let dir = repository("comparison-mutable-checkout")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "same\n")])?;
    let head = Workspace::discover(&root)?.head_commit().context("head")?;
    let other = repository("comparison-mutable-other")?;
    let other_root = other.0.join("ws");
    git::commit_and_stage(&other_root, &[("a.md", "same\n")])?;
    let other_checkout = Workspace::discover(&other_root)?.identity();
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let working = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "other working tree",
        )
        .with_working_tree_facts(WorkingTreeFacts::new(
            Some(head.clone()),
            WorkingTreeState::Clean,
            Some(ContentIdentity::from_text("same\n")),
            other_checkout.clone(),
            FullFileDigest::from_bytes(b"same\n"),
        )),
        "same\n",
        1,
    )?;
    let index = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "other index",
        )
        .with_index_facts(IndexFacts::new(
            Some(head),
            IndexState::Unchanged,
            Some(ContentIdentity::from_text("same\n")),
            other_checkout,
            FullFileDigest::from_bytes(b"same\n"),
        )),
        "same\n",
        2,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.settle_background();
    for id in [&working, &index] {
        assert!(!app.normal_thread(app.thread(id).context("foreign checkout")?));
    }
    app.set_comparison_target(ComparisonEndpoint::Index);
    app.settle_background();
    assert!(!app.normal_thread(app.thread(&index).context("foreign index")?));
    Ok(())
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one target-only fixture covers every typed endpoint and both landing sides"
)]
fn diff_off_uses_only_typed_target_content_and_keeps_annotations_content_only() -> anyhow::Result<()>
{
    let dir = repository("comparison-off-membership")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "parent\n")])?;
    git::commit_and_stage(&root, &[("a.md", "child\n")])?;
    let child = Workspace::discover(&root)?.head_commit().context("child")?;
    let checkout = Workspace::discover(&root)?.identity();
    let content = ContentIdentity::from_text("child\n");
    let digest = FullFileDigest::from_bytes(b"child\n");
    let working_content = ContentIdentity::from_text("working\n");
    let working_digest = FullFileDigest::from_bytes(b"working\n");
    let index_content = ContentIdentity::from_text("index\n");
    let index_digest = FullFileDigest::from_bytes(b"index\n");
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let commit_thread = |body: &str, side| {
        Draft::new(Author::User, Path::new("a.md"), LineRange::new(1, 1), body)
            .at_source(OriginVersion::commit(&child), side)
    };
    let base = store.annotate(commit_thread("base", OriginSide::Base), "child\n", 1)?;
    let target = store.annotate(commit_thread("target", OriginSide::Target), "child\n", 2)?;
    let unspecified = store.annotate(
        commit_thread("unspecified", OriginSide::Unspecified),
        "child\n",
        3,
    )?;
    let working = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "working",
        )
        .at_source(
            OriginVersion::working_tree(Some(child.clone())),
            OriginSide::Target,
        )
        .with_working_tree_facts(WorkingTreeFacts::new(
            Some(child.clone()),
            WorkingTreeState::Clean,
            Some(working_content.clone()),
            checkout.clone(),
            working_digest.clone(),
        )),
        "working\n",
        4,
    )?;
    let index = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "index",
        )
        .at_source(
            OriginVersion::index(Some(child.clone())),
            OriginSide::Target,
        )
        .with_index_facts(IndexFacts::new(
            Some(child.clone()),
            IndexState::Unchanged,
            Some(index_content),
            checkout.clone(),
            index_digest,
        )),
        "index\n",
        5,
    )?;
    let landed_target = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "landed target",
        )
        .at_source(
            OriginVersion::working_tree(Some(child.clone())),
            OriginSide::Target,
        )
        .with_working_tree_facts(WorkingTreeFacts::new(
            Some(child.clone()),
            WorkingTreeState::Clean,
            Some(content.clone()),
            checkout.clone(),
            digest.clone(),
        )),
        "child\n",
        6,
    )?;
    let landed_base = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "landed base",
        )
        .at_source(
            OriginVersion::working_tree(Some(child.clone())),
            OriginSide::Base,
        )
        .with_working_tree_facts(WorkingTreeFacts::new(
            Some(child.clone()),
            WorkingTreeState::Clean,
            Some(content),
            checkout,
            digest,
        )),
        "child\n",
        7,
    )?;
    for id in [&landed_target, &landed_base] {
        let candidate = store
            .thread(id)
            .and_then(fathomable_core::annotations::Thread::landing_candidate)
            .context("landing candidate")?;
        assert_eq!(
            store.land(&candidate, &CommitId::parse(&child)?)?,
            fathomable_core::annotations::LandingOutcome::Applied
        );
    }
    fs::write(root.join("a.md"), "working\n")?;
    git::stage(&root, &[("a.md", "index\n")])?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.open(Path::new("a.md"));
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&child)?));
    app.settle_background();
    let mark_ids = app
        .marks()
        .iter()
        .map(super::super::threads::Mark::id)
        .collect::<Vec<_>>();
    for id in [&target, &unspecified] {
        assert!(mark_ids.contains(&id));
    }
    for (name, id) in [
        ("base", &base),
        ("working", &working),
        ("index", &index),
        ("landed target", &landed_target),
        ("landed base", &landed_base),
    ] {
        assert!(!mark_ids.contains(&id), "{name} unexpectedly rendered");
    }

    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    app.settle_background();
    assert!(app.normal_thread(app.thread(&working).context("working")?));
    assert!(app.marks().iter().all(|mark| mark.id() != &target));
    app.set_comparison_target(ComparisonEndpoint::Index);
    app.settle_background();
    assert!(app.normal_thread(app.thread(&index).context("index")?));
    assert!(!app.normal_thread(app.thread(&working).context("working")?));

    Ok(())
}

#[test]
fn commit_review_uses_first_parent_and_rejects_an_unavailable_parent() -> anyhow::Result<()> {
    let dir = repository("comparison-first-parent")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "root\n")])?;
    let alternate = Workspace::discover(&root)?
        .head_commit()
        .context("alternate")?;
    git::commit_and_stage(&root, &[("a.md", "first parent\n")])?;
    let first_parent = Workspace::discover(&root)?
        .head_commit()
        .context("first parent")?;
    let merge = git::merge_commit_and_stage(&root, &[("a.md", "merge\n")], &alternate)?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.select_commit_parent(&CommitId::parse(&merge)?, None);
    app.settle_background();
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&first_parent)?)
    );
    assert_ne!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&alternate)?)
    );
    drop(app);

    let object = root
        .join(".git/objects")
        .join(&first_parent[..2])
        .join(&first_parent[2..]);
    fs::remove_file(object)?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    let before = (
        app.comparison.base().clone(),
        app.comparison.target().clone(),
    );
    app.select_commit_parent(&CommitId::parse(&merge)?, None);
    assert_eq!(
        (app.comparison.base(), app.comparison.target()),
        (&before.0, &before.1)
    );
    assert!(
        app.message()
            .is_some_and(|message| message.contains("cannot read selected commit's first parent"))
    );
    Ok(())
}

#[test]
fn off_target_context_is_absent_while_immutable_target_scans() -> anyhow::Result<()> {
    let dir = repository("comparison-off-accepted-target")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let head = Workspace::discover(&root)?.head_commit().context("head")?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let id = store.annotate(
        Draft::new(
            Author::agent("reviewer"),
            Path::new("a.md"),
            LineRange::new(1, 1),
            "target",
        )
        .at_source(OriginVersion::commit(&head), OriginSide::Unspecified),
        "one\n",
        1,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.settle_background();
    app.open(Path::new("a.md"));
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&head)?));
    assert!(app.marks().iter().all(|mark| mark.id() != &id));
    app.settle_background();
    assert!(app.marks().iter().any(|mark| mark.id() == &id));
    Ok(())
}

#[test]
fn failed_target_refresh_keeps_membership_on_the_accepted_presentation() -> anyhow::Result<()> {
    let dir = repository("comparison-failed-target-membership")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let accepted = Workspace::discover(&root)?.head_commit().context("head")?;
    let missing = "1111111111111111111111111111111111111111";
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let accepted_thread = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "accepted",
        )
        .at_source(OriginVersion::commit(&accepted), OriginSide::Target),
        "one\n",
        1,
    )?;
    let requested_thread = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "requested",
        )
        .at_source(OriginVersion::commit(missing), OriginSide::Target),
        "missing\n",
        2,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::EmptyTree);
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&accepted)?));
    app.settle_background();
    assert!(app.normal_thread(app.thread(&accepted_thread).context("accepted")?));

    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(missing)?));
    assert!(app.normal_thread(app.thread(&accepted_thread).context("pending accepted")?));
    assert!(!app.normal_thread(app.thread(&requested_thread).context("pending requested")?));
    app.settle_background();
    assert!(app.comparison.error().is_some());
    assert!(app.normal_thread(app.thread(&accepted_thread).context("failed accepted")?));
    assert!(!app.normal_thread(app.thread(&requested_thread).context("failed requested")?));
    Ok(())
}

#[test]
fn format_two_comparison_preferences_are_rejected_without_rewrite() -> anyhow::Result<()> {
    let dir = repository("comparison-old-preference")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let state = dir.0.join("state").into_os_string();
    let dirs = XdgDirs::resolve(|name| (name == "XDG_STATE_HOME").then(|| state.clone()));
    let first_dirs = dirs.clone();
    drop(
        AppBuilder::at(&root)
            .unopened()
            .options(move |mut options| {
                options.dirs = first_dirs;
                options
            })
            .build()?,
    );
    let preference = dirs.comparison_dir(&root).join("comparison.json");
    let old = br#"{"version":2,"source":{"intent":"follow-head"},"target":"working-tree","source_alias":{"kind":"head"},"whitespace":false,"focus":null}"#;
    fs::write(&preference, old)?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.dirs = dirs;
            options
        })
        .build()?;
    app.settle_background();
    assert!(
        app.comparison
            .error()
            .is_some_and(|error| error.contains("format is unsupported"))
    );
    assert_eq!(fs::read(preference)?, old);
    Ok(())
}

#[test]
fn head_to_working_tree_reselects_the_current_head() -> anyhow::Result<()> {
    let dir = repository("comparison-head-working-tree")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("first"))?;
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    let head = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("head"))?;
    fs::write(root.join("a.md"), "working\n")?;

    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    app.act(Action::ComparisonHeadWorkingTree);
    app.settle_background();

    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&head)?)
    );
    assert_eq!(app.comparison.target(), &ComparisonEndpoint::WorkingTree);
    assert_eq!(
        app.comparison.base_alias(),
        Some(&super::EndpointAlias::Head)
    );
    assert_eq!(
        app.comparison_menu_pair(),
        ("HEAD".to_owned(), "WorkingTree".to_owned())
    );
    assert_eq!(app.diff_mode(), DiffMode::Normal);
    Ok(())
}

#[test]
fn unified_presentation_rebuilds_across_files_and_comparisons() -> anyhow::Result<()> {
    let dir = repository("comparison-global-unified")?;
    let root = dir.0.join("ws");
    fs::write(root.join("a.txt"), "a0\n")?;
    fs::write(root.join("b.txt"), "b0\n")?;
    git::commit_and_stage(&root, &[("a.txt", "a0\n"), ("b.txt", "b0\n")])?;
    let first = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("first"))?;
    fs::write(root.join("a.txt"), "a1\n")?;
    fs::write(root.join("b.txt"), "b1\n")?;
    git::commit_and_stage(&root, &[("a.txt", "a1\n"), ("b.txt", "b1\n")])?;
    let second = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("second"))?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.settle_background();
    app.open(Path::new("a.txt"));
    app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
    app.settle_background();
    app.open(Path::new("b.txt"));
    assert!(app.view().diff_view());
    assert!(
        app.view()
            .layout()
            .lines()
            .iter()
            .any(|line| line.text().contains("b1"))
    );

    fs::write(root.join("a.txt"), "working-a\n")?;
    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    app.settle_background();
    app.open(Path::new("a.txt"));
    assert!(app.view().diff_view());
    assert!(
        app.view()
            .layout()
            .lines()
            .iter()
            .any(|line| line.text().contains("working-a"))
    );
    Ok(())
}
