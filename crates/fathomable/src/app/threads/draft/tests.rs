use std::fs;
use std::path::Path;

use anyhow::Context as _;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use fathomable_core::annotations::{
    Author, AutoResolve, Draft, Lifecycle, LineRange, MessageTarget, OriginSide, OriginVersion,
    Store, Thread,
};
use fathomable_core::editor::{Edit, Motion};
use fathomable_core::workspace::{CommitId, ComparisonEndpoint, Workspace};
use fathomable_testing::{TempDir, git};

use super::{ComposeTarget, displayed_diff_side_and_range, mixed_diff_selection};
use crate::app::App;
use crate::app::diff::{DiffBody, DiffView, Side, Text};
use crate::app::input::bindings::Where;
use crate::app::input::keys;
use crate::app::testing::{self, click, press, press_key, screen};

fn loose_blob_path(root: &Path, revision: &str, path: &str) -> anyhow::Result<std::path::PathBuf> {
    let mut command = std::process::Command::new("git");
    for variable in [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_PREFIX",
        "GIT_WORK_TREE",
    ] {
        command.env_remove(variable);
    }
    let output = command
        .arg("-C")
        .arg(root)
        .args(["rev-parse", &format!("{revision}:{path}")])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "cannot resolve test blob: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let id = String::from_utf8(output.stdout)?;
    let id = id.trim();
    anyhow::ensure!(id.len() == 40, "unexpected test blob id");
    Ok(root.join(".git/objects").join(&id[..2]).join(&id[2..]))
}

#[test]
fn historical_deleted_source_comments_keep_base_origin() -> anyhow::Result<()> {
    let dir = TempDir::new("historical-deleted-origin")?;
    git::init(&dir.0)?;
    fs::write(dir.0.join("gone.md"), "original line\n")?;
    git::commit_and_stage(&dir.0, &[("gone.md", "original line\n")])?;
    let first = Workspace::discover(&dir.0)?
        .head_commit()
        .context("first commit")?;
    fs::remove_file(dir.0.join("gone.md"))?;
    git::commit_and_stage(&dir.0, &[])?;
    let second = Workspace::discover(&dir.0)?
        .head_commit()
        .context("second commit")?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.settle_background();
    app.open(Path::new("gone.md"));
    app.start_new_comment();
    anyhow::ensure!(
        matches!(app.popup(), Some(crate::app::Popup::Compose(_))),
        "comment draft did not open: {:?}",
        app.message()
    );
    press(&mut app, "historical finding");
    app.compose_submit();

    let id = {
        let thread = app
            .store
            .as_ref()
            .and_then(|store| store.threads().first())
            .context("historical thread")?;
        assert_eq!(thread.origin_side(), OriginSide::Base);
        assert_eq!(thread.origin().range(), Some(LineRange::new(1, 1)));
        assert_eq!(thread.origin().snippet(), "original line");
        assert_eq!(thread.origin_version(), &OriginVersion::commit(first));
        thread.id().clone()
    };

    app.goto_message(id.clone(), 0);
    app.thread_reply();
    assert!(matches!(app.popup(), Some(crate::app::Popup::Compose(_))));
    press(&mut app, "historical reply");
    app.compose_submit();

    app.goto_message(id, 0);
    app.thread_edit_message();
    assert!(matches!(app.popup(), Some(crate::app::Popup::Compose(_))));
    Ok(())
}

#[test]
fn switching_an_open_working_file_to_history_captures_displayed_text() -> anyhow::Result<()> {
    let dir = TempDir::new("open-then-historical-origin")?;
    git::init(&dir.0)?;
    fs::write(dir.0.join("a.txt"), "historical\n")?;
    git::commit_and_stage(&dir.0, &[("a.txt", "historical\n")])?;
    let historical = Workspace::discover(&dir.0)?
        .head_commit()
        .context("historical commit")?;
    fs::write(dir.0.join("a.txt"), "working\nshort\n")?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.open(Path::new("a.txt"));
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&historical)?));
    app.settle_background();
    app.start_new_comment();
    press(&mut app, "displayed history");
    app.compose_submit();

    let thread = app
        .store
        .as_ref()
        .and_then(|store| store.threads().first())
        .context("historical thread")?;
    assert_eq!(thread.origin().snippet(), "historical");
    assert_eq!(thread.origin_version(), &OriginVersion::commit(historical));
    Ok(())
}

#[test]
fn pending_and_failed_target_change_cannot_create_mismatched_provenance() -> anyhow::Result<()> {
    let dir = TempDir::new("pending-target-annotation")?;
    git::init(&dir.0)?;
    git::commit_and_stage(&dir.0, &[("a.txt", "old a\n"), ("b.txt", "old b\n")])?;
    let old = Workspace::discover(&dir.0)?
        .head_commit()
        .context("old commit")?;
    git::commit_and_stage(&dir.0, &[("a.txt", "new a\n"), ("b.txt", "new b\n")])?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.settle_background();
    app.open(Path::new("a.txt"));
    assert_eq!(app.view().text(), "new a\n");
    let mut limits = app.workspace.limits().clone();
    limits.comparison_paths = 1;
    app.workspace.set_limits(limits);

    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&old)?));
    assert!(app.comparison.pending());
    app.start_new_comment();
    assert!(!matches!(app.popup(), Some(crate::app::Popup::Compose(_))));
    assert!(
        app.message()
            .is_some_and(|message| message.contains("projection is not ready"))
    );

    app.settle_background();
    assert!(
        app.comparison
            .error()
            .is_some_and(|error| error.contains("limited"))
    );
    assert_eq!(app.view().text(), "new a\n");
    app.start_new_comment();
    app.compose_insert("must not be stored");
    app.compose_submit();
    assert!(
        app.store
            .as_ref()
            .is_some_and(|store| store.threads().is_empty())
    );

    app.select_diff_mode(fathomable_core::config::DiffMode::Off);
    assert!(app.comparison.pending());
    app.start_file_comment();
    assert!(!matches!(app.popup(), Some(crate::app::Popup::Compose(_))));
    app.settle_background();
    app.start_file_comment();
    assert!(!matches!(app.popup(), Some(crate::app::Popup::Compose(_))));
    Ok(())
}

#[test]
fn active_and_parked_drafts_freeze_pending_projection_evidence() -> anyhow::Result<()> {
    let dir = TempDir::new("draft-projection-freeze")?;
    git::init(&dir.0)?;
    git::commit_and_stage(&dir.0, &[("a.txt", "old a\n"), ("b.txt", "old b\n")])?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.settle_background();

    app.open(Path::new("a.txt"));
    fs::write(dir.0.join("a.txt"), "new a\n")?;
    app.refresh_comparison();
    assert!(app.comparison.pending());
    app.start_new_comment();
    assert!(!app.comparison.pending());
    press(&mut app, "active evidence");
    app.refresh_comparison();
    assert!(!app.comparison.pending());
    let _ = app.poll_background();
    assert_eq!(app.view().text(), "old a\n");
    app.comparison
        .select_target_aliased(ComparisonEndpoint::EmptyTree, None);
    app.compose_submit();
    assert!(matches!(app.popup(), Some(crate::app::Popup::Compose(_))));
    assert!(
        app.store
            .as_ref()
            .is_some_and(|store| store.threads().is_empty())
    );
    app.comparison
        .select_target_aliased(ComparisonEndpoint::WorkingTree, None);
    app.compose_submit();
    assert!(app.comparison.pending());
    app.settle_background();

    app.open(Path::new("b.txt"));
    fs::write(dir.0.join("b.txt"), "new b\n")?;
    app.start_new_comment();
    assert!(!app.comparison.pending());
    press(&mut app, "parked evidence");
    app.open(Path::new("a.txt"));
    app.refresh_comparison();
    assert!(!app.comparison.pending());
    let _ = app.poll_background();
    let parked = app
        .docs
        .iter()
        .find(|doc| doc.relative == Path::new("b.txt"))
        .context("parked document")?;
    assert_eq!(parked.view.text(), "old b\n");

    let store = app.store.as_ref().context("thread store")?;
    assert_eq!(store.threads().len(), 1);
    assert_eq!(store.threads()[0].origin().snippet(), "old a");
    Ok(())
}

#[test]
fn draft_freeze_still_lands_each_observed_head_in_order() -> anyhow::Result<()> {
    let dir = TempDir::new("draft-landing-head-order")?;
    git::init(&dir.0)?;
    git::commit_and_stage(&dir.0, &[("a.txt", "initial\n")])?;
    fs::write(dir.0.join("a.txt"), "land me\n")?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.settle_background();
    let initial = Workspace::discover(&dir.0)?
        .head_commit()
        .context("initial commit")?;
    app.open(Path::new("a.txt"));
    app.start_file_comment();
    press(&mut app, "candidate");
    app.compose_submit();
    let id = app
        .store
        .as_ref()
        .and_then(|store| store.threads().first())
        .map(Thread::id)
        .cloned()
        .context("candidate thread")?;

    app.start_file_comment();
    press(&mut app, "keep comparison frozen");
    git::commit_and_stage(&dir.0, &[("a.txt", "land me\n")])?;
    let commit_b = Workspace::discover(&dir.0)?
        .head_commit()
        .context("commit B")?;
    app.refresh_comparison();
    git::commit_and_stage(&dir.0, &[("a.txt", "later\n")])?;
    let commit_c = Workspace::discover(&dir.0)?
        .head_commit()
        .context("commit C")?;
    app.refresh_comparison();
    app.settle_background();

    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(&initial)?),
        "the draft keeps its installed Source selected"
    );
    assert_eq!(
        app.thread(&id).and_then(Thread::landed_commit),
        Some(commit_b.as_str())
    );
    assert!(!app.comparison.pending());
    app.compose_submit();
    app.settle_background();
    assert!(app.draft().is_none(), "the preserved draft submits");
    assert_eq!(
        app.comparison.base(),
        &ComparisonEndpoint::Commit(CommitId::parse(commit_c)?),
        "the deferred transition applies after the draft closes"
    );
    assert_eq!(
        app.store.as_ref().map(|store| store.threads().len()),
        Some(2)
    );
    Ok(())
}

#[test]
fn pending_off_to_active_restore_blocks_new_annotations() -> anyhow::Result<()> {
    let dir = TempDir::new("draft-pending-mode-restore")?;
    git::init(&dir.0)?;
    git::commit_and_stage(&dir.0, &[("a.txt", "one\n")])?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.settle_background();
    app.open(Path::new("a.txt"));
    app.select_diff_mode(fathomable_core::config::DiffMode::Off);
    app.settle_background();

    app.set_comparison_base(ComparisonEndpoint::EmptyTree);
    assert!(app.comparison.restore_mode.is_some());
    app.start_new_comment();

    assert!(!matches!(app.popup(), Some(crate::app::Popup::Compose(_))));
    assert!(app.comparison.pending());
    app.settle_background();
    assert_ne!(app.diff_mode(), fathomable_core::config::DiffMode::Off);
    Ok(())
}

#[test]
fn failed_active_projection_read_blocks_annotation_on_retained_text() -> anyhow::Result<()> {
    let dir = TempDir::new("draft-failed-active-projection")?;
    git::init(&dir.0)?;
    git::commit_and_stage(&dir.0, &[("a.txt", "one\n")])?;
    let first = Workspace::discover(&dir.0)?
        .head_commit()
        .context("first commit")?;
    git::commit_and_stage(&dir.0, &[("a.txt", "two\n")])?;
    let second = Workspace::discover(&dir.0)?
        .head_commit()
        .context("second commit")?;
    git::commit_and_stage(&dir.0, &[("a.txt", "three\n")])?;
    let third = Workspace::discover(&dir.0)?
        .head_commit()
        .context("third commit")?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.settle_background();
    app.open(Path::new("a.txt"));
    assert_eq!(app.view().text(), "two\n");

    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&third)?));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !app.comparison.poll() {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "comparison did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    anyhow::ensure!(app.comparison.error().is_none(), "comparison failed");
    fs::remove_file(loose_blob_path(&dir.0, &third, "a.txt")?)?;
    let index = app.current.context("current document")?;
    app.apply_comparison_projection(index);

    assert_eq!(app.view().text(), "two\n");
    assert!(!app.view().comparison_projection_ready());
    assert!(app.docs[index].comparison_notice.is_some());
    app.start_new_comment();
    assert!(!matches!(app.popup(), Some(crate::app::Popup::Compose(_))));
    Ok(())
}

#[test]
fn failed_off_target_read_blocks_file_annotation() -> anyhow::Result<()> {
    let dir = TempDir::new("draft-failed-off-projection")?;
    git::init(&dir.0)?;
    git::commit_and_stage(&dir.0, &[("a.txt", "one\n")])?;
    let first = Workspace::discover(&dir.0)?
        .head_commit()
        .context("first commit")?;
    git::commit_and_stage(&dir.0, &[("a.txt", "two\n")])?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.settle_background();
    app.open(Path::new("a.txt"));
    app.select_diff_mode(fathomable_core::config::DiffMode::Off);
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&first)?));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let paths = loop {
        if let Some(result) = app.comparison.poll_paths() {
            break result.map_err(anyhow::Error::msg)?;
        }
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "Target discovery did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    app.off_target_paths = Some(paths);
    fs::remove_file(loose_blob_path(&dir.0, &first, "a.txt")?)?;
    app.finish_off_target();

    let index = app.current.context("current document")?;
    assert!(app.docs[index].comparison_notice.is_some());
    app.start_file_comment();
    assert!(!matches!(app.popup(), Some(crate::app::Popup::Compose(_))));
    Ok(())
}

#[test]
fn off_branch_historical_target_projects_inline_without_rescoping() -> anyhow::Result<()> {
    let dir = TempDir::new("off-branch-historical-origin")?;
    let main = dir.0.join("main");
    fs::create_dir(&main)?;
    git::init(&main)?;
    fs::write(main.join("a.txt"), "main\n")?;
    git::commit_and_stage(&main, &[("a.txt", "main\n")])?;
    let base = Workspace::discover(&main)?
        .head_commit()
        .context("main commit")?;
    let feature = dir.0.join("feature");
    git::worktree_add(&main, &feature, "feature")?;
    fs::write(feature.join("a.txt"), "feature\n")?;
    git::commit_and_stage(&feature, &[("a.txt", "feature\n")])?;
    let target = Workspace::discover(&feature)?
        .head_commit()
        .context("feature commit")?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&main)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(base)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.settle_background();
    app.open(Path::new("a.txt"));
    app.start_new_comment();
    press(&mut app, "feature discussion");
    app.compose_submit();

    let thread = app
        .store
        .as_ref()
        .and_then(|store| store.threads().first())
        .context("feature thread")?;
    assert!(!app.reach.here(thread));
    assert_eq!(thread.origin_version(), &OriginVersion::commit(target));
    assert_eq!(app.marks().len(), 1);
    assert_eq!(app.marks()[0].range(), Some(LineRange::new(1, 1)));
    Ok(())
}

#[test]
fn exact_off_branch_thread_uses_installed_projection_not_other_worktree() -> anyhow::Result<()> {
    let dir = TempDir::new("off-branch-exact-placement")?;
    let main = dir.0.join("main");
    fs::create_dir(&main)?;
    git::init(&main)?;
    git::commit_and_stage(&main, &[("a.txt", "main\n")])?;
    let base = Workspace::discover(&main)?
        .head_commit()
        .context("main commit")?;
    let feature = dir.0.join("feature");
    git::worktree_add(&main, &feature, "feature")?;
    git::commit_and_stage(&feature, &[("a.txt", "one\ntarget\n")])?;
    let target = Workspace::discover(&feature)?
        .head_commit()
        .context("feature commit")?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&main)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(base)?));
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.settle_background();
    app.open(Path::new("a.txt"));
    app.view_mut().goto_row(1);
    app.start_new_comment();
    press(&mut app, "exact feature line");
    app.compose_submit();

    fs::write(feature.join("a.txt"), "shifted\none\ntarget\n")?;
    app.refresh_reach();
    let thread = app
        .store
        .as_ref()
        .and_then(|store| store.threads().first())
        .context("feature thread")?;
    assert!(app.elsewhere_placement(thread.id()).is_some());
    let entries = app.normal_review_entries(false);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].range(), Some(LineRange::new(2, 2)));
    Ok(())
}

#[test]
fn unopened_exact_thread_in_off_mode_uses_the_installed_target() -> anyhow::Result<()> {
    let dir = TempDir::new("off-installed-target-placement")?;
    git::init(&dir.0)?;
    let a_text = "target\npad\n";
    let b_text = "pad\ntarget\n";
    git::commit_and_stage(&dir.0, &[("a.txt", a_text)])?;
    let a = Workspace::discover(&dir.0)?
        .head_commit()
        .context("A commit")?;
    git::commit_and_stage(&dir.0, &[("a.txt", b_text)])?;
    let b = Workspace::discover(&dir.0)?
        .head_commit()
        .context("B commit")?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.txt"),
            LineRange::new(2, 2),
            "B placement",
        )
        .at_source(OriginVersion::commit(&b), OriginSide::Target),
        b_text,
        1,
    )?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::EmptyTree);
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&a)?));
    app.settle_background();
    app.select_diff_mode(fathomable_core::config::DiffMode::Off);
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&b)?));
    app.settle_background();

    assert!(app.docs.is_empty());
    let entries = app.normal_review_entries(false);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].range(), Some(LineRange::new(2, 2)));
    Ok(())
}

#[test]
fn duplicate_removed_lines_keep_the_selected_base_range_in_origin() -> anyhow::Result<()> {
    let dir = TempDir::new("duplicate-removed-origin")?;
    git::init(&dir.0)?;
    let old = "same\nremove\nsame\nremove\n";
    let new = "same\nsame\n";
    git::commit_and_stage(&dir.0, &[("a.md", old)])?;
    let first = Workspace::discover(&dir.0)?
        .head_commit()
        .context("first commit")?;
    git::commit_and_stage(&dir.0, &[("a.md", new)])?;
    let second = Workspace::discover(&dir.0)?
        .head_commit()
        .context("second commit")?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.settle_background();
    app.open(Path::new("a.md"));
    app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
    app.settle_background();
    let row = app
        .view()
        .layout()
        .lines()
        .iter()
        .position(|line| line.diff_old_line() == Some(4) && line.diff_new_line().is_none())
        .context("second removed line")?;
    app.view_mut().goto_row(row);
    app.start_new_comment();
    press(&mut app, "second duplicate");
    app.compose_submit();

    let thread = app
        .store
        .as_ref()
        .and_then(|store| store.threads().first())
        .context("removed-line thread")?;
    assert_eq!(thread.origin_side(), OriginSide::Base);
    assert_eq!(thread.origin().range(), Some(LineRange::new(4, 4)));
    assert_eq!(thread.origin().snippet(), "remove");
    assert_eq!(thread.origin_version(), &OriginVersion::commit(first));
    Ok(())
}

#[test]
fn removed_unified_line_draft_blocks_mode_and_endpoint_changes_when_parked() -> anyhow::Result<()> {
    let dir = TempDir::new("removed-line-draft-guard")?;
    git::init(&dir.0)?;
    git::commit_and_stage(&dir.0, &[("a.md", "keep\nremove\n"), ("b.md", "other\n")])?;
    let first = Workspace::discover(&dir.0)?
        .head_commit()
        .context("first commit")?;
    git::commit_and_stage(&dir.0, &[("a.md", "keep\n"), ("b.md", "other\n")])?;
    let second = Workspace::discover(&dir.0)?
        .head_commit()
        .context("second commit")?;
    let store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    let base = ComparisonEndpoint::Commit(CommitId::parse(&first)?);
    let target = ComparisonEndpoint::Commit(CommitId::parse(&second)?);
    app.set_comparison_base(base.clone());
    app.settle_background();
    app.set_comparison_target(target.clone());
    app.settle_background();
    app.open(Path::new("a.md"));
    app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
    app.settle_background();
    let row = app
        .view()
        .layout()
        .lines()
        .iter()
        .position(|line| line.diff_old_line() == Some(2) && line.diff_new_line().is_none())
        .context("removed line")?;
    app.view_mut().goto_row(row);
    app.start_new_comment();
    press(&mut app, "pending removed-line note");

    app.select_diff_mode(fathomable_core::config::DiffMode::Off);
    app.settle_background();
    assert_eq!(app.diff_mode(), fathomable_core::config::DiffMode::Unified);
    assert!(
        app.message()
            .is_some_and(|message| message.contains("submit or cancel"))
    );

    app.open(Path::new("b.md"));
    assert!(!matches!(app.popup(), Some(crate::app::Popup::Compose(_))));
    app.select_diff_mode(fathomable_core::config::DiffMode::Normal);
    app.settle_background();
    app.set_comparison_base(target.clone());
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    app.settle_background();
    assert_eq!(app.diff_mode(), fathomable_core::config::DiffMode::Unified);
    assert_eq!(app.comparison.base(), &base);
    assert_eq!(app.comparison.target(), &target);
    assert!(
        app.docs
            .iter()
            .any(|doc| doc.relative == Path::new("a.md") && doc.draft.is_some())
    );
    Ok(())
}

#[test]
fn working_edits_do_not_relocate_threads_in_an_immutable_comparison() -> anyhow::Result<()> {
    let dir = TempDir::new("immutable-comparison-reload")?;
    git::init(&dir.0)?;
    fs::write(dir.0.join("a.txt"), "one\ntwo\n")?;
    git::commit_and_stage(&dir.0, &[("a.txt", "one\ntwo\n")])?;
    let first = Workspace::discover(&dir.0)?
        .head_commit()
        .context("first commit")?;
    fs::write(dir.0.join("a.txt"), "one\nTWO\n")?;
    git::commit_and_stage(&dir.0, &[("a.txt", "one\nTWO\n")])?;
    let second = Workspace::discover(&dir.0)?
        .head_commit()
        .context("second commit")?;
    let store_path = dir.0.join("threads.jsonl");
    let store = Store::open(&store_path)?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.settle_background();
    app.open(Path::new("a.txt"));
    app.view_mut().goto_source_line(2);
    app.start_new_comment();
    press(&mut app, "historical target");
    app.compose_submit();
    let id = app
        .store
        .as_ref()
        .and_then(|store| store.threads().first())
        .map(|thread| thread.id().clone())
        .context("historical target thread")?;
    let before = fs::read(&store_path)?;

    fs::write(dir.0.join("a.txt"), "unrelated\nworking\nrewrite\n")?;
    app.on_changes(vec![dir.0.join("a.txt")]);

    assert_eq!(app.view().text(), "one\nTWO\n");
    assert_eq!(app.marks()[0].range(), Some(LineRange::new(2, 2)));
    assert_eq!(fs::read(&store_path)?, before);
    assert_eq!(
        app.thread(&id).and_then(Thread::range),
        Some(LineRange::new(2, 2))
    );
    Ok(())
}

#[test]
fn working_renames_do_not_rewrite_immutable_comparison_paths() -> anyhow::Result<()> {
    let dir = TempDir::new("immutable-comparison-rename")?;
    git::init(&dir.0)?;
    fs::write(dir.0.join("a.txt"), "one\n")?;
    git::commit_and_stage(&dir.0, &[("a.txt", "one\n")])?;
    let first = Workspace::discover(&dir.0)?
        .head_commit()
        .context("first commit")?;
    fs::write(dir.0.join("a.txt"), "two\n")?;
    git::commit_and_stage(&dir.0, &[("a.txt", "two\n")])?;
    let second = Workspace::discover(&dir.0)?
        .head_commit()
        .context("second commit")?;
    let store_path = dir.0.join("threads.jsonl");
    let store = Store::open(&store_path)?;
    let mut app = testing::AppBuilder::at(&dir.0)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&second)?));
    app.settle_background();
    app.open(Path::new("a.txt"));
    app.start_new_comment();
    press(&mut app, "historical target");
    app.compose_submit();
    let before = fs::read(&store_path)?;

    fs::rename(dir.0.join("a.txt"), dir.0.join("b.txt"))?;
    app.on_events(vec![crate::app::watch::Event::Renamed {
        from: dir.0.join("a.txt"),
        to: dir.0.join("b.txt"),
    }]);
    app.settle_background();

    assert_eq!(app.current_path(), Path::new("a.txt"));
    assert_eq!(app.view().text(), "two\n");
    assert_eq!(app.marks().len(), 1);
    assert_eq!(fs::read(&store_path)?, before);
    assert_eq!(
        app.store
            .as_ref()
            .and_then(|store| store.threads().first())
            .map(Thread::path),
        Some(Path::new("a.txt"))
    );
    Ok(())
}

#[test]
fn removed_diff_rows_keep_exact_original_line_identity() -> anyhow::Result<()> {
    let old = "same\nremove\nsame\nremove\n";
    let new = "same\nsame\n";
    let mut view = crate::app::view::View::new(new.to_owned(), 80, 20);
    view.show_diff(DiffView {
        base: Side::ComparisonBase,
        target: Side::ComparisonTarget,
        badge: "DIFF comparison".to_owned(),
        body: DiffBody::Diff {
            base: Text::Owned(old.to_owned()),
            target: Text::Owned(new.to_owned()),
        },
    });
    let row = view
        .layout()
        .lines()
        .iter()
        .position(|line| line.diff_old_line() == Some(4) && line.diff_new_line().is_none())
        .context("second duplicate removed line")?;
    view.goto_row(row);
    assert_eq!(
        displayed_diff_side_and_range(&view),
        Some((
            fathomable_core::annotations::OriginSide::Base,
            LineRange::new(4, 4)
        ))
    );
    Ok(())
}

#[test]
fn a_diff_selection_must_stay_on_one_side() -> anyhow::Result<()> {
    let old = "keep\nfirst\nsecond\nkeep\n";
    let new = "keep\nreplacement\nkeep\n";
    let mut view = crate::app::view::View::new(new.to_owned(), 80, 20);
    view.show_diff(DiffView {
        base: Side::ComparisonBase,
        target: Side::ComparisonTarget,
        badge: "DIFF comparison".to_owned(),
        body: DiffBody::Diff {
            base: Text::Owned(old.to_owned()),
            target: Text::Owned(new.to_owned()),
        },
    });
    let removed: Vec<usize> = view
        .layout()
        .lines()
        .iter()
        .enumerate()
        .filter_map(|(row, line)| {
            (line.diff_old_line().is_some() && line.diff_new_line().is_none()).then_some(row)
        })
        .collect();
    view.goto_row(removed[0]);
    view.select_lines();
    view.move_down(1);
    let selection = view.selection().context("removed selection")?;
    assert!(!mixed_diff_selection(&view, selection));
    assert_eq!(
        displayed_diff_side_and_range(&view),
        Some((
            fathomable_core::annotations::OriginSide::Base,
            LineRange::new(2, 3)
        ))
    );

    view.move_down(1);
    let selection = view.selection().context("mixed selection")?;
    assert!(mixed_diff_selection(&view, selection));
    Ok(())
}

fn click_file(app: &mut App, path: &str) -> anyhow::Result<()> {
    let row = app
        .tree()
        .context("files pane is hidden")?
        .rows()
        .iter()
        .position(|row| row.path() == Path::new(path))
        .context("file missing from tree")?;
    click(app, 2, row + 1);
    assert_eq!(app.current_path(), Path::new(path));
    Ok(())
}

#[test]
fn drafts_stay_in_their_files_when_clicking_away_and_back() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-file-click", testing::README)?;
    fs::write(dir.0.join("ws/main.c"), "first\nsecond\nthird\nfourth\n")?;
    let mut app = testing::source_app(&dir)?;
    app.show_tree();
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    press(&mut app, "README draft");
    app.compose_edit(Edit::Move(Motion::Left));
    let cursor = app.draft().context("README draft")?.buffer().cursor();
    assert!(screen(&app)?.join("\n").contains("README draft"));

    click_file(&mut app, "main.c")?;
    assert!(app.draft().is_none(), "the README draft is not active here");
    assert_eq!(keys::place(&app), Some(Where::Tree));
    assert!(app.draft_cursor_cell().is_none());
    assert!(!screen(&app)?.join("\n").contains("README draft"));
    press_key(&mut app, KeyCode::Enter);
    assert!(Store::open(testing::store_path(&dir))?.threads().is_empty());

    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    press(&mut app, "main draft");
    click_file(&mut app, "README.md")?;
    assert!(app.draft().is_none(), "preview keeps drafts parked");
    press_key(&mut app, KeyCode::Enter);
    let draft = app.draft().context("restored README draft")?;
    assert_eq!(draft.buffer().text(), "README draft");
    assert_eq!(draft.buffer().cursor(), cursor);
    assert_eq!(draft.target(), &ComposeTarget::New(LineRange::new(3, 3)));
    assert_eq!(keys::place(&app), Some(Where::Draft));
    let shown = screen(&app)?.join("\n");
    assert!(shown.contains("README draft"));
    assert!(!shown.contains("main draft"));
    press_key(&mut app, KeyCode::Enter);
    assert!(app.draft().is_none());

    click_file(&mut app, "main.c")?;
    assert!(app.draft().is_none(), "preview keeps drafts parked");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.compose_draft(), Some("main draft"));
    press_key(&mut app, KeyCode::Enter);
    click_file(&mut app, "README.md")?;
    assert!(app.draft().is_none(), "submitted drafts do not return");
    let store = Store::open(testing::store_path(&dir))?;
    assert_eq!(store.threads().len(), 2);
    let readme = store
        .threads()
        .iter()
        .find(|thread| thread.path() == Path::new("README.md"))
        .context("README thread")?;
    assert_eq!(readme.comment(), "README draft");
    assert_eq!(readme.range(), Some(LineRange::new(3, 3)));
    assert_eq!(readme.snippet(), "alpha");
    let main = store
        .threads()
        .iter()
        .find(|thread| thread.path() == Path::new("main.c"))
        .context("main thread")?;
    assert_eq!(main.comment(), "main draft");
    assert_eq!(main.snippet(), "third");
    Ok(())
}

#[test]
fn file_drafts_survive_navigation_and_cancel_only_in_their_file() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-file-navigation", testing::README)?;
    fs::write(dir.0.join("ws/main.c"), "main\n")?;
    let mut app = testing::source_app(&dir)?;
    app.start_file_comment();
    press(&mut app, "whole README");
    app.compose_cancel();
    assert!(app.draft().context("draft")?.confirming_discard());

    for path in ["main.c", "README.md"] {
        app.open(Path::new(path));
        assert_eq!(app.current_path(), Path::new(path));
        if path == "main.c" {
            assert!(app.draft().is_none());
            app.compose_submit();
            app.start_file_comment();
            press(&mut app, "whole main");
        }
    }
    assert_eq!(app.compose_draft(), Some("whole README"));
    assert!(app.draft().context("restored draft")?.confirming_discard());
    assert!(screen(&app)?.join("\n").contains("comment on README.md"));
    app.compose_cancel();
    app.open(Path::new("main.c"));
    assert_eq!(app.compose_draft(), Some("whole main"));
    app.compose_submit();
    app.open(Path::new("README.md"));
    assert!(app.draft().is_none(), "cancelled draft does not return");

    let store = Store::open(testing::store_path(&dir))?;
    assert_eq!(store.threads().len(), 1);
    assert_eq!(store.threads()[0].path(), Path::new("main.c"));
    assert_eq!(store.threads()[0].range(), None);
    assert_eq!(store.threads()[0].comment(), "whole main");
    Ok(())
}

#[test]
fn reply_and_edit_drafts_resume_in_their_original_thread() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-thread-navigation", testing::README)?;
    fs::write(dir.0.join("ws/main.c"), "main\n")?;
    let mut app = testing::source_app(&dir)?;
    app.start_new_comment();
    press(&mut app, "opening");
    app.compose_submit();
    let id = app.marks()[0].id().clone();

    for target in [
        ComposeTarget::Reply(id.clone()),
        ComposeTarget::Edit {
            thread: id.clone(),
            message: MessageTarget::Comment,
        },
    ] {
        app.open_compose(target.clone());
        app.set_compose_text("changed");
        app.open(Path::new("main.c"));
        assert!(app.draft().is_none());
        assert!(app.draft_cursor_cell().is_none());
        app.compose_submit();
        assert_eq!(app.thread(&id).context("thread")?.comment(), "opening");
        app.open(Path::new("README.md"));
        assert_eq!(app.draft().context("restored draft")?.target(), &target);
        assert_eq!(app.compose_draft(), Some("changed"));
        assert!(app.draft_cursor_cell().is_some());
        assert!(screen(&app)?.join("\n").contains("changed"));
        app.compose_submit();
    }
    let thread = app.thread(&id).context("thread")?;
    assert_eq!(thread.comment(), "changed");
    assert_eq!(thread.replies().len(), 1);
    assert_eq!(thread.replies()[0].body(), "changed");
    Ok(())
}

#[test]
fn reopening_the_same_file_or_failing_to_open_keeps_the_draft() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-open-unchanged", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.start_new_comment();
    press(&mut app, "keep me");
    for path in ["README.md", "missing.c", "README.md"] {
        app.open(Path::new(path));
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.compose_draft(), Some("keep me"));
    }
    app.compose_submit();
    assert_eq!(app.marks().len(), 1);
    Ok(())
}

#[test]
fn starting_a_reply_from_elsewhere_does_not_replace_a_parked_draft() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-reply-collision", testing::README)?;
    fs::write(dir.0.join("ws/main.c"), "main\n")?;
    let mut app = testing::source_app(&dir)?;
    app.start_new_comment();
    press(&mut app, "opening");
    app.compose_submit();
    let id = app.marks()[0].id().clone();
    app.start_file_comment();
    press(&mut app, "keep this draft");
    app.open(Path::new("main.c"));

    app.open_compose(ComposeTarget::Reply(id));
    assert_eq!(app.current_path(), Path::new("README.md"));
    assert_eq!(app.compose_draft(), Some("keep this draft"));
    assert_eq!(
        app.draft().context("original draft")?.target(),
        &ComposeTarget::OnFile
    );
    assert_eq!(
        app.message(),
        Some("finish or discard this file's draft first")
    );
    Ok(())
}

#[test]
fn a_parked_draft_follows_its_documents_rename() -> anyhow::Result<()> {
    use crate::app::watch::Event;

    let dir = testing::workspace("draft-rename", testing::README)?;
    fs::write(dir.0.join("ws/main.c"), "main\n")?;
    let mut app = testing::source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    press(&mut app, "keep with file");
    app.open(Path::new("main.c"));
    let from = dir.0.join("ws/README.md");
    let to = dir.0.join("ws/GUIDE.md");
    fs::rename(&from, &to)?;
    app.on_events(vec![Event::Renamed { from, to }]);
    app.settle_background();
    app.open(Path::new("GUIDE.md"));
    assert_eq!(app.compose_draft(), Some("keep with file"));
    app.compose_submit();

    let store = Store::open(testing::store_path(&dir))?;
    assert_eq!(store.threads().len(), 1);
    assert_eq!(store.threads()[0].path(), Path::new("GUIDE.md"));
    assert_eq!(store.threads()[0].range(), Some(LineRange::new(3, 3)));
    assert_eq!(store.threads()[0].snippet(), "alpha");
    Ok(())
}

#[test]
fn a_resolved_reply_draft_requires_confirmation_and_preserves_ctrl_enter() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-resolved-reply", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    press(&mut app, "opening");
    app.compose_submit();
    let id = app.marks()[0].id().clone();
    app.open_compose(ComposeTarget::Reply(id.clone()));
    press(&mut app, "kept reply");

    let mut other = Store::open(testing::store_path(&dir))?;
    other.resolve(&id, Some("abc123"), 20)?;

    app.compose_submit_auto_resolve();
    assert_eq!(app.compose_draft(), Some("kept reply"));
    assert!(app.draft().context("draft")?.confirming_reopen());
    let stored = Store::open(testing::store_path(&dir))?;
    let thread = stored.thread(&id).context("thread")?;
    assert_eq!(thread.lifecycle(), Lifecycle::Resolved);
    assert!(thread.replies().is_empty());
    assert!(screen(&app)?.join("\n").contains("resolved while editing"));

    app.compose_cancel();
    assert_eq!(app.compose_draft(), Some("kept reply"));
    assert!(!app.draft().context("draft")?.confirming_reopen());
    app.compose_submit_auto_resolve();
    app.compose_submit();

    let stored = Store::open(testing::store_path(&dir))?;
    let thread = stored.thread(&id).context("thread")?;
    assert_eq!(thread.lifecycle(), Lifecycle::Active);
    assert_eq!(thread.auto_resolve(), AutoResolve::Enabled);
    assert_eq!(thread.replies().len(), 1);
    assert_eq!(thread.replies()[0].body(), "kept reply");
    assert!(app.draft().is_none());
    Ok(())
}

#[test]
fn a_resolved_edit_draft_reopens_and_saves_only_after_enter() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-resolved-edit", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    press(&mut app, "opening");
    app.compose_submit();
    let id = app.marks()[0].id().clone();
    app.open_compose(ComposeTarget::Edit {
        thread: id.clone(),
        message: MessageTarget::Comment,
    });
    app.set_compose_text("edited");

    let mut other = Store::open(testing::store_path(&dir))?;
    other.resolve(&id, None, 20)?;
    app.compose_submit();
    assert_eq!(app.compose_draft(), Some("edited"));
    assert!(app.draft().context("draft")?.confirming_reopen());
    assert_eq!(
        Store::open(testing::store_path(&dir))?
            .thread(&id)
            .context("thread")?
            .comment(),
        "opening"
    );

    app.compose_submit();
    let stored = Store::open(testing::store_path(&dir))?;
    let thread = stored.thread(&id).context("thread")?;
    assert_eq!(thread.lifecycle(), Lifecycle::Active);
    assert_eq!(thread.comment(), "edited");
    assert!(app.draft().is_none());
    Ok(())
}

#[test]
fn ctrl_enter_submits_with_auto_resolve_and_alt_enter_is_newline() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-submit-keys", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    press(&mut app, "first");
    keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    press(&mut app, "second");
    assert_eq!(app.compose_draft(), Some("first\nsecond"));
    keys::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL),
    );
    let thread = app
        .marks()
        .first()
        .and_then(|mark| app.thread(mark.id()))
        .context("thread")?;
    assert_eq!(thread.comment(), "first\nsecond");
    assert_eq!(thread.auto_resolve(), AutoResolve::Enabled);
    Ok(())
}

#[test]
fn shift_enter_adds_newlines_to_comment_reply_and_edit_drafts() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-shift-enter", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    press(&mut app, "comment first");
    keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    press(&mut app, "comment second");
    assert_eq!(
        app.compose_draft(),
        Some("comment first\ncomment second"),
        "Shift-Enter must not submit a new comment"
    );
    press_key(&mut app, KeyCode::Enter);

    let id = app.marks()[0].id().clone();
    app.open_compose(ComposeTarget::Reply(id.clone()));
    press(&mut app, "reply first");
    keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    press(&mut app, "reply second");
    assert_eq!(
        app.compose_draft(),
        Some("reply first\nreply second"),
        "Shift-Enter must not submit a reply"
    );
    press_key(&mut app, KeyCode::Enter);

    app.open_compose(ComposeTarget::Edit {
        thread: id,
        message: MessageTarget::Comment,
    });
    keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
    press(&mut app, "edited tail");
    assert_eq!(
        app.compose_draft(),
        Some("comment first\ncomment second\nedited tail"),
        "Shift-Enter must not save an edit"
    );
    press_key(&mut app, KeyCode::Enter);
    assert!(
        app.draft().is_none(),
        "plain Enter must still save the edit"
    );
    Ok(())
}

#[test]
fn a_persistence_error_keeps_the_draft_intact() -> anyhow::Result<()> {
    let dir = testing::workspace("draft-write-error", testing::README)?;
    let mut app = testing::source_app(&dir)?;
    app.view_mut().goto_source_line(3);
    app.start_new_comment();
    press(&mut app, "opening");
    app.compose_submit();
    app.view_mut().goto_source_line(5);
    app.start_new_comment();
    press(&mut app, "keep after failure");

    let path = testing::store_path(&dir);
    let aside = path.with_extension("aside");
    fs::rename(&path, &aside)?;
    fs::create_dir(&path)?;
    app.compose_submit();
    assert_eq!(app.compose_draft(), Some("keep after failure"));
    assert!(
        app.message()
            .is_some_and(|message| message.contains("cannot save comment"))
    );

    fs::remove_dir(&path)?;
    fs::rename(aside, path)?;
    Ok(())
}
