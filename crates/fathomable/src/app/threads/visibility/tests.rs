use std::path::Path;

use fathomable_core::annotations::{Author, Draft, LineRange, OriginSide, OriginVersion, Store};
use fathomable_core::config::DiffMode;
use fathomable_core::review_points::ReviewPointStore;
use fathomable_core::workspace::{CommitId, ComparisonEndpoint, Workspace};
use fathomable_testing::git;

use crate::app::testing::{self, AppBuilder};

fn head(root: &Path) -> anyhow::Result<String> {
    Workspace::discover(root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("HEAD is missing"))
}

fn commit_thread(
    store: &mut Store,
    commit: &str,
    side: OriginSide,
    comment: &str,
    now: u64,
) -> anyhow::Result<fathomable_core::annotations::ThreadId> {
    Ok(store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            comment,
        )
        .at_source(OriginVersion::commit(commit), side),
        "evidence\n",
        now,
    )?)
}

#[test]
fn normal_membership_adds_the_accepted_commit_range_without_widening_inline_placement()
-> anyhow::Result<()> {
    let dir = testing::bare("thread-commit-range-linear")?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("a.md", "a\n")])?;
    let first = head(&root)?;
    git::commit_and_stage(&root, &[("a.md", "b\n")])?;
    let middle = head(&root)?;
    git::commit_and_stage(&root, &[("a.md", "c\n")])?;
    let target = head(&root)?;
    let mut store = Store::open(testing::store_path(&dir))?;
    let finding = commit_thread(
        &mut store,
        &middle,
        OriginSide::Unspecified,
        "middle finding",
        1,
    )?;
    let base_finding = commit_thread(&mut store, &middle, OriginSide::Base, "base evidence", 2)?;
    let original_version = store
        .thread(&finding)
        .ok_or_else(|| anyhow::anyhow!("finding"))?
        .origin_version()
        .clone();
    drop(store);

    let mut app = AppBuilder::new(&dir).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.settle_background();

    let thread = app
        .thread(&finding)
        .ok_or_else(|| anyhow::anyhow!("finding"))?;
    assert!(app.normal_thread_without_draft(thread));
    assert!(!app.thread_matches_presentation(thread));
    assert!(!app.inline_thread(thread));
    let base_thread = app
        .thread(&base_finding)
        .ok_or_else(|| anyhow::anyhow!("base finding"))?;
    assert!(app.normal_thread_without_draft(base_thread));
    assert!(!app.inline_thread(base_thread));
    assert_eq!(thread.id(), &finding);
    assert_eq!(thread.origin_version(), &original_version);

    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    app.settle_background();
    assert!(
        app.normal_thread_without_draft(
            app.thread(&finding)
                .ok_or_else(|| anyhow::anyhow!("working-tree finding"))?
        )
    );
    app.set_comparison_target(ComparisonEndpoint::Index);
    app.settle_background();
    assert!(
        app.normal_thread_without_draft(
            app.thread(&finding)
                .ok_or_else(|| anyhow::anyhow!("index finding"))?
        )
    );

    app.set_comparison_base(ComparisonEndpoint::WorkingTree);
    app.settle_background();
    assert!(
        !app.normal_thread_without_draft(
            app.thread(&finding)
                .ok_or_else(|| anyhow::anyhow!("working-tree source finding"))?
        )
    );
    app.set_comparison_base(ComparisonEndpoint::Index);
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.settle_background();
    assert!(
        !app.normal_thread_without_draft(
            app.thread(&finding)
                .ok_or_else(|| anyhow::anyhow!("index source finding"))?
        )
    );

    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&middle)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.settle_background();
    assert!(
        !app.normal_thread_without_draft(
            app.thread(&finding)
                .ok_or_else(|| anyhow::anyhow!("narrowed finding"))?
        )
    );

    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.select_diff_mode(DiffMode::Off);
    app.settle_background();
    assert!(
        !app.normal_thread_without_draft(
            app.thread(&finding)
                .ok_or_else(|| anyhow::anyhow!("off finding"))?
        )
    );
    Ok(())
}

#[test]
fn accepted_range_survives_pending_and_failed_replacements() -> anyhow::Result<()> {
    let dir = testing::bare("thread-commit-range-accepted")?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("a.md", "a\n")])?;
    let first = head(&root)?;
    git::commit_and_stage(&root, &[("a.md", "b\n")])?;
    let middle = head(&root)?;
    git::commit_and_stage(&root, &[("a.md", "c\n")])?;
    let target = head(&root)?;
    let mut store = Store::open(testing::store_path(&dir))?;
    let finding = commit_thread(
        &mut store,
        &middle,
        OriginSide::Unspecified,
        "middle finding",
        1,
    )?;
    drop(store);
    let mut app = AppBuilder::new(&dir).unopened().build()?;
    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&first)?));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    app.settle_background();

    app.set_comparison_base(ComparisonEndpoint::Commit(CommitId::parse(&target)?));
    assert!(app.comparison.pending());
    assert!(
        app.normal_thread_without_draft(
            app.thread(&finding)
                .ok_or_else(|| anyhow::anyhow!("pending finding"))?
        )
    );

    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(
        "1111111111111111111111111111111111111111",
    )?));
    app.settle_background();
    assert!(app.comparison.error().is_some());
    assert!(
        app.normal_thread_without_draft(
            app.thread(&finding)
                .ok_or_else(|| anyhow::anyhow!("failed finding"))?
        )
    );
    Ok(())
}

#[test]
fn review_point_source_uses_its_stored_baseline() -> anyhow::Result<()> {
    let dir = testing::bare("thread-commit-range-review-point")?;
    let root = testing::root(&dir);
    git::init(&root)?;
    git::commit_and_stage(&root, &[("a.md", "a\n")])?;
    git::commit_and_stage(&root, &[("a.md", "b\n")])?;
    let baseline = head(&root)?;
    let points = dir.0.join("state/review-points");
    let mut point_store = ReviewPointStore::open(&points)?;
    let mut workspace = Workspace::discover(&root)?;
    let point = point_store
        .capture(&mut workspace, Some("baseline"))?
        .point()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("review point"))?;
    git::commit_and_stage(&root, &[("a.md", "c\n")])?;
    let target = head(&root)?;
    let mut store = Store::open(testing::store_path(&dir))?;
    let baseline_finding = commit_thread(
        &mut store,
        &baseline,
        OriginSide::Unspecified,
        "baseline finding",
        1,
    )?;
    let target_finding = commit_thread(
        &mut store,
        &target,
        OriginSide::Unspecified,
        "target finding",
        2,
    )?;
    drop(store);

    let mut app = AppBuilder::new(&dir)
        .unopened()
        .review_points(&points)
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::ReviewPoint(point.id().to_owned()));
    app.settle_background();
    app.set_comparison_target(ComparisonEndpoint::WorkingTree);
    app.settle_background();
    assert!(
        !app.normal_thread_without_draft(
            app.thread(&baseline_finding)
                .ok_or_else(|| anyhow::anyhow!("baseline finding"))?
        )
    );
    assert!(
        app.normal_thread_without_draft(
            app.thread(&target_finding)
                .ok_or_else(|| anyhow::anyhow!("target finding"))?
        )
    );
    Ok(())
}
