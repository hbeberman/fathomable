use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::Context as _;
use fathomable_core::XdgDirs;
use fathomable_core::config::DiffMode;
use fathomable_core::content::Content;
use fathomable_core::tree::Rule;
use fathomable_core::workspace::{CommitId, ComparisonEndpoint, Filter, Workspace};
use fathomable_testing::{TempDir, git};

use crate::app::input::bindings::Action;
use crate::app::testing::{AppBuilder, press, press_key};
use crate::app::{PickerKind, Popup};
use crossterm::event::KeyCode;

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
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
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
    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    assert!(app.status().is_empty());
    app.open(Path::new("gone.md"));
    assert_eq!(app.view().text(), "from A\n");
    assert_eq!(app.banner(), Some("deleted in comparison · showing base"));
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
    press(&mut app, " ds");
    assert!(!app.view().diff_view());
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
fn parentless_head_does_not_change_the_comparison() -> anyhow::Result<()> {
    let dir = repository("comparison-parentless")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root).unopened().build()?;
    let before = (
        app.comparison.base().clone(),
        app.comparison.target().clone(),
    );
    let refreshes = app.comparison.refresh_count();

    press(&mut app, " dl");

    assert_eq!(app.comparison.refresh_count(), refreshes);
    assert_eq!(
        (app.comparison.base(), app.comparison.target()),
        (&before.0, &before.1)
    );
    assert!(
        app.message()
            .is_some_and(|message| message.contains("has no parent"))
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
    assert_eq!(off.last_active_diff_mode, DiffMode::Standard);
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
    assert!(app.view().diff_view());
    press(&mut app, " vs");
    assert_eq!(app.diff_mode(), DiffMode::Unified);
    assert!(app.view().diff_view());
    assert!(
        app.message()
            .is_some_and(|message| message.contains("unavailable in unified"))
    );
    app.select_diff_mode(DiffMode::Standard);
    assert!(
        app.view().source_view(),
        "the source choice returns with the standard view"
    );
    app.select_diff_mode(DiffMode::Off);
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
    assert!(app.view().diff_view());
    app.open(Path::new("README.md"));
    assert!(
        app.view().diff_view(),
        "file switches keep unified presentation"
    );
    app.select_diff_mode(DiffMode::Standard);
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
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.open(Path::new("gone.md"));
    assert_eq!(app.view().text(), "base secret\n");
    app.select_diff_mode(DiffMode::Unified);
    app.select_diff_mode(DiffMode::Off);

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
    app.select_diff_mode(DiffMode::Standard);
    assert_eq!(app.view().text(), "base secret\n");
    assert_eq!(app.banner(), Some("deleted in comparison · showing base"));
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
    assert_eq!(app.view().text(), "must not survive\n");

    app.select_diff_mode(DiffMode::Off);
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
    assert_eq!(app.view().cursor_source_line(), line);
    assert_eq!(app.view().scroll(), scroll);
    app.refresh_comparison();
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
    assert_eq!(app.view().diff_counts(), None);

    app.select_diff_mode(DiffMode::Standard);
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
    app.select_diff_mode(DiffMode::Off);
    app.set_comparison_base(ComparisonEndpoint::ReviewPoint("point".to_owned()));
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
    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert_eq!(app.view().text(), "one\n");
    assert!(app.comparison.error().is_none());
    app.select_diff_mode(DiffMode::Unified);
    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert_eq!(
        app.comparison.target(),
        &ComparisonEndpoint::Commit(CommitId::parse(&first)?)
    );

    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    fs::write(root.join("a.txt"), "working\n")?;
    let current = app
        .current
        .ok_or_else(|| anyhow::anyhow!("current document"))?;
    assert!(app.reload_doc(current).is_some());
    assert_eq!(app.view().text(), "working\n");

    let unavailable =
        ComparisonEndpoint::Commit(CommitId::parse("0000000000000000000000000000000000000000")?);
    app.set_comparison_base(unavailable.clone());
    assert_eq!(app.diff_mode(), DiffMode::Off);
    assert_eq!(app.comparison.base(), &unavailable);
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    assert_eq!(app.diff_mode(), DiffMode::Standard);
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
fn saving_a_point_restores_diff_mode_and_selects_working_tree() -> anyhow::Result<()> {
    let dir = repository("comparison-point-select-off")?;
    let root = dir.0.join("ws");
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .review_points(dir.0.join("points"))
        .build()?;
    app.select_diff_mode(DiffMode::Off);

    app.save_review_point(None);

    assert_eq!(app.diff_mode(), DiffMode::Standard);
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
    let point = app
        .review_points
        .as_ref()
        .and_then(|store| store.list().first().cloned())
        .ok_or_else(|| anyhow::anyhow!("saved point"))?;
    fs::write(root.join("a.md"), "working\n")?;
    app.set_comparison_base(ComparisonEndpoint::ReviewPoint(point.id().to_owned()));

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
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(second)?));
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
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(second)?));
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
    assert!(
        app.tree()
            .is_some_and(|tree| tree.contains(Path::new("current-only.md")))
    );
    Ok(())
}

#[test]
fn untouched_default_base_survives_restart_after_head_moves() -> anyhow::Result<()> {
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

    fs::write(root.join("a.md"), "two\n")?;
    git::commit_and_stage(&root, &[("a.md", "two\n")])?;
    let app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.dirs = dirs;
            options
        })
        .build()?;
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&first)?)
    );
    assert_eq!(app.comparison_menu_pair().0, first[..7]);
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
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.select_diff_mode(DiffMode::Off);
    app.act(Action::ComparisonHeadWorkingTree);

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
    assert_eq!(app.diff_mode(), DiffMode::Standard);
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
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.open(Path::new("a.txt"));
    app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
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
