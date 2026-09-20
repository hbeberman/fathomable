use std::fs;
use std::path::Path;

use fathomable_core::annotations::{
    Author, ContentIdentity, Draft, FullFileDigest, Lifecycle, LineRange, Store, ThreadId,
    WorkingTreeFacts, WorkingTreeState,
};
use fathomable_core::workspace::{CommitId, ComparisonEndpoint, Workspace};
use fathomable_testing::{TempDir, git};

use super::{App, Cancellation, Landing, Request, ResultSet, reconcile};
use crate::app::testing::AppBuilder;

fn paged_app(name: &str) -> anyhow::Result<(TempDir, App, Vec<ThreadId>, CommitId)> {
    let dir = TempDir::new(name)?;
    let root = dir.0.join("ws");
    git::init(&root)?;
    git::commit_and_stage(
        &root,
        &[("a.md", "aaa\n"), ("b.md", "bbb\n"), ("c.md", "ccc\n")],
    )?;
    let workspace = Workspace::discover(&root)?;
    let head = workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("HEAD"))?;
    let checkout = workspace.identity();
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let mut ids = Vec::new();
    for (created, (path, text)) in [
        (1, ("a.md", "aaa\n")),
        (2, ("b.md", "bbb\n")),
        (3, ("c.md", "ccc\n")),
    ] {
        ids.push(
            store.annotate(
                Draft::new(Author::User, Path::new(path), LineRange::new(1, 1), path)
                    .with_working_tree_facts(WorkingTreeFacts::new(
                        Some(head.clone()),
                        WorkingTreeState::Modified,
                        Some(ContentIdentity::from_text(text)),
                        checkout.clone(),
                        FullFileDigest::from_bytes(text.as_bytes()),
                    )),
                text,
                created,
            )?,
        );
    }
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(|mut options| {
            options.store = None;
            options.limits.comparison_paths = 1;
            options
        })
        .build()?;
    app.store = Some(store);
    Ok((dir, app, ids, CommitId::parse(&head)?))
}

fn first_page(app: &App, commit: &CommitId) -> anyhow::Result<ResultSet> {
    let context = app
        .landing_context(Some(commit.clone()), app.workspace.limits().clone())
        .map_err(anyhow::Error::msg)?
        .ok_or_else(|| anyhow::anyhow!("landing context"))?;
    let request = app
        .landing_request(context, None)
        .ok_or_else(|| anyhow::anyhow!("first landing page"))?;
    Ok(reconcile(request, Cancellation::default()))
}

fn next_page(app: &App, result: ResultSet) -> anyhow::Result<ResultSet> {
    let continuation = result
        .continuation
        .ok_or_else(|| anyhow::anyhow!("landing continuation"))?;
    let request = app
        .landing_request(continuation.context, Some(&continuation.after))
        .ok_or_else(|| anyhow::anyhow!("next landing page"))?;
    Ok(reconcile(request, Cancellation::default()))
}

fn request_at(app: &App, commit: &CommitId) -> anyhow::Result<Request> {
    let context = app
        .landing_context(Some(commit.clone()), app.workspace.limits().clone())
        .map_err(anyhow::Error::msg)?
        .ok_or_else(|| anyhow::anyhow!("landing context"))?;
    app.landing_request(context, None)
        .ok_or_else(|| anyhow::anyhow!("landing request"))
}

fn request_commit(request: &Request) -> Option<&CommitId> {
    request
        .context
        .captures
        .first()
        .and_then(|capture| capture.commit.as_ref())
}

#[test]
fn saturated_queue_retains_oldest_and_newest_requests() -> anyhow::Result<()> {
    let (_dir, app, _ids, _) = paged_app("landing-saturated-queue")?;
    let first = CommitId::parse("1111111111111111111111111111111111111111")?;
    let middle = CommitId::parse("2222222222222222222222222222222222222222")?;
    let latest = CommitId::parse("3333333333333333333333333333333333333333")?;
    let mut landing = Landing::new();

    landing.retain_later(request_at(&app, &first)?);
    landing.retain_later(request_at(&app, &middle)?);
    landing.retain_later(request_at(&app, &latest)?);

    let next = landing
        .next_request(None)
        .ok_or_else(|| anyhow::anyhow!("oldest request"))?;
    assert_eq!(request_commit(&next), Some(&first));
    assert_eq!(
        landing.queued.as_ref().and_then(request_commit),
        Some(&latest)
    );
    assert!(landing.retry.is_none());

    let last = landing
        .next_request(None)
        .ok_or_else(|| anyhow::anyhow!("latest request"))?;
    assert_eq!(request_commit(&last), Some(&latest));
    assert!(!landing.pending());
    Ok(())
}

#[test]
fn local_trigger_binds_head_before_worker_execution() -> anyhow::Result<()> {
    let dir = TempDir::new("landing-local-head-capture")?;
    let root = dir.0.join("ws");
    git::init(&root)?;
    git::commit_and_stage(&root, &[("a.md", "captured\n")])?;
    let workspace = Workspace::discover(&root)?;
    let captured = workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("captured HEAD"))?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "candidate",
        )
        .with_working_tree_facts(WorkingTreeFacts::new(
            Some(captured.clone()),
            WorkingTreeState::Modified,
            Some(ContentIdentity::from_text("captured\n")),
            workspace.identity(),
            FullFileDigest::from_bytes(b"captured\n"),
        )),
        "captured\n",
        1,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(|mut options| {
            options.store = None;
            options
        })
        .build()?;
    app.store = Some(store);
    let context = app
        .landing_context(None, app.workspace.limits().clone())
        .map_err(anyhow::Error::msg)?
        .ok_or_else(|| anyhow::anyhow!("landing context"))?;
    let request = app
        .landing_request(context, None)
        .ok_or_else(|| anyhow::anyhow!("landing request"))?;

    git::commit_and_stage(&root, &[("a.md", "later\n")])?;
    let result = reconcile(request, Cancellation::default());

    assert_eq!(result.matches.len(), 1);
    assert_eq!(result.matches[0].candidate.thread(), &id);
    assert_eq!(result.matches[0].commit.as_str(), captured);
    Ok(())
}

#[test]
fn startup_reconciliation_lands_archived_exact_content() -> anyhow::Result<()> {
    let dir = TempDir::new("landing-archived")?;
    let root = dir.0.join("ws");
    git::init(&root)?;
    git::commit_and_stage(&root, &[("a.md", "one\n")])?;
    let workspace = Workspace::discover(&root)?;
    let head = workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("HEAD"))?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "mutable origin",
        )
        .with_working_tree_facts(WorkingTreeFacts::new(
            Some(head.clone()),
            WorkingTreeState::Modified,
            Some(ContentIdentity::from_text("one\n")),
            workspace.identity(),
            FullFileDigest::from_bytes(b"one\n"),
        )),
        "one\n",
        1,
    )?;
    store.archive(&id, 2)?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.trigger_landing(Some(CommitId::parse(&head)?));
    app.settle_background();

    let thread = app.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?;
    assert!(thread.is_archived());
    assert_eq!(thread.landed_commit(), Some(head.as_str()));
    Ok(())
}

#[test]
fn bounded_reconciliation_advances_past_an_older_mismatch() -> anyhow::Result<()> {
    let dir = TempDir::new("landing-fair-continuation")?;
    let root = dir.0.join("ws");
    git::init(&root)?;
    git::commit_and_stage(&root, &[("a.md", "not old\n"), ("b.md", "exact\n")])?;
    let workspace = Workspace::discover(&root)?;
    let head = workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("HEAD"))?;
    let checkout = workspace.identity();
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let facts = |digest| {
        WorkingTreeFacts::new(
            Some(head.clone()),
            WorkingTreeState::Modified,
            Some(ContentIdentity::from_text(digest)),
            checkout.clone(),
            FullFileDigest::from_bytes(digest.as_bytes()),
        )
    };
    let older = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "older mismatch",
        )
        .with_working_tree_facts(facts("old\n")),
        "old\n",
        1,
    )?;
    let newer = store.annotate(
        Draft::new(
            Author::User,
            Path::new("b.md"),
            LineRange::new(1, 1),
            "newer match",
        )
        .with_working_tree_facts(facts("exact\n")),
        "exact\n",
        2,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options.limits.comparison_paths = 1;
            options
        })
        .build()?;
    app.trigger_landing(Some(CommitId::parse(&head)?));
    app.settle_background();

    assert_eq!(
        app.thread(&older).and_then(|thread| thread.landed_commit()),
        None
    );
    assert_eq!(
        app.thread(&newer).and_then(|thread| thread.landed_commit()),
        Some(head.as_str())
    );
    Ok(())
}

#[test]
fn retained_path_limit_pages_every_eligible_candidate() -> anyhow::Result<()> {
    let (_dir, mut app, ids, commit) = paged_app("landing-effective-path-limit")?;
    app.store
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("store"))?
        .delete(&ids[2], 10)?;
    let mut limits = app.workspace.limits().clone();
    limits.retained_paths = 1;
    limits.comparison_paths = 2;
    app.workspace.set_limits(limits);

    app.trigger_landing(Some(commit.clone()));
    app.settle_background();

    for id in &ids[..2] {
        assert_eq!(
            app.thread(id).and_then(|thread| thread.landed_commit()),
            Some(commit.as_str())
        );
    }
    Ok(())
}

#[test]
fn path_page_keeps_non_active_checkout_commit_across_head_and_root_changes() -> anyhow::Result<()> {
    let dir = TempDir::new("landing-page-captures")?;
    let main = dir.0.join("main");
    git::init(&main)?;
    git::commit_and_stage(&main, &[("main.md", "main\n"), ("other.md", "base\n")])?;
    let main_head = Workspace::discover(&main)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("main HEAD"))?;
    let feature = dir.0.join("feature");
    git::worktree_add(&main, &feature, "feature")?;
    git::commit_and_stage(&feature, &[("other.md", "captured\n")])?;
    let feature_workspace = Workspace::discover(&feature)?;
    let feature_head = feature_workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("feature HEAD"))?;

    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let add = |store: &mut Store, checkout, head: &str, path: &str, text: &str, created| {
        store.annotate(
            Draft::new(Author::User, Path::new(path), LineRange::new(1, 1), path)
                .with_working_tree_facts(WorkingTreeFacts::new(
                    Some(head.to_owned()),
                    WorkingTreeState::Modified,
                    Some(ContentIdentity::from_text(text)),
                    checkout,
                    FullFileDigest::from_bytes(text.as_bytes()),
                )),
            text,
            created,
        )
    };
    let main_id = add(
        &mut store,
        Workspace::discover(&main)?.identity(),
        &main_head,
        "main.md",
        "main\n",
        1,
    )?;
    let feature_id = add(
        &mut store,
        feature_workspace.identity(),
        &feature_head,
        "other.md",
        "captured\n",
        2,
    )?;
    let mut app = AppBuilder::at(&main)
        .unopened()
        .options(|mut options| {
            options.store = None;
            options.limits.comparison_paths = 1;
            options
        })
        .build()?;
    app.store = Some(store);
    let mut bounded = app.workspace.limits().clone();
    bounded.discovery_entries = 1;
    assert!(
        app.landing_context(None, bounded)
            .is_err_and(|error| error.contains("discovery limit"))
    );
    let context = app
        .landing_context(None, app.workspace.limits().clone())
        .map_err(anyhow::Error::msg)?
        .ok_or_else(|| anyhow::anyhow!("landing context"))?;
    let request = app
        .landing_request(context, None)
        .ok_or_else(|| anyhow::anyhow!("first landing page"))?;
    let first = reconcile(request, Cancellation::default());
    assert_eq!(first.matches[0].candidate.thread(), &main_id);
    assert_eq!(first.matches[0].commit.as_str(), main_head);
    assert!(first.continuation.as_ref().is_some_and(|continuation| {
        continuation.context.captures.iter().any(|capture| {
            capture.checkout == feature_workspace.identity()
                && capture.commit.as_ref().map(CommitId::as_str) == Some(feature_head.as_str())
        })
    }));

    git::commit_and_stage(&feature, &[("other.md", "advanced\n")])?;
    assert!(app.activate_worktree(&feature));
    let second = next_page(&app, first)?;
    assert_eq!(second.matches[0].candidate.thread(), &feature_id);
    assert_eq!(second.matches[0].commit.as_str(), feature_head);
    Ok(())
}

#[test]
fn deleting_a_page_boundary_does_not_skip_later_candidates() -> anyhow::Result<()> {
    let (_dir, mut app, ids, commit) = paged_app("landing-page-delete")?;
    let first = first_page(&app, &commit)?;
    assert_eq!(first.matches[0].candidate.thread(), &ids[0]);
    app.store
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("store"))?
        .delete(&ids[0], 10)?;
    let second = next_page(&app, first)?;
    assert_eq!(second.matches[0].candidate.thread(), &ids[1]);
    let matched = &second.matches[0];
    app.store
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("store"))?
        .land(&matched.candidate, &matched.commit)?;
    assert_eq!(
        app.thread(&ids[1])
            .and_then(|thread| thread.landed_commit()),
        Some(commit.as_str())
    );
    Ok(())
}

#[test]
fn archiving_a_page_boundary_does_not_skip_later_candidates() -> anyhow::Result<()> {
    let (_dir, mut app, ids, commit) = paged_app("landing-page-archive")?;
    let first = first_page(&app, &commit)?;
    assert_eq!(first.matches[0].candidate.thread(), &ids[0]);
    let store = app.store.as_mut().ok_or_else(|| anyhow::anyhow!("store"))?;
    store.resolve(&ids[0], Some(commit.as_str()), 10)?;
    store.archive(&ids[0], 11)?;
    let second = next_page(&app, first)?;
    assert_eq!(second.matches[0].candidate.thread(), &ids[1]);
    let matched = &second.matches[0];
    app.store
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("store"))?
        .land(&matched.candidate, &matched.commit)?;
    assert_eq!(
        app.thread(&ids[1])
            .and_then(|thread| thread.landed_commit()),
        Some(commit.as_str())
    );
    Ok(())
}

#[test]
fn byte_bounded_reconciliation_resumes_after_an_older_mismatch() -> anyhow::Result<()> {
    let dir = TempDir::new("landing-byte-continuation")?;
    let root = dir.0.join("ws");
    git::init(&root)?;
    git::commit_and_stage(&root, &[("a.md", "bad\n"), ("b.md", "new\n")])?;
    let workspace = Workspace::discover(&root)?;
    let head = workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("HEAD"))?;
    let checkout = workspace.identity();
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let facts = |text: &str| {
        WorkingTreeFacts::new(
            Some(head.clone()),
            WorkingTreeState::Modified,
            Some(ContentIdentity::from_text(text)),
            checkout.clone(),
            FullFileDigest::from_bytes(text.as_bytes()),
        )
    };
    let older = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "older mismatch",
        )
        .with_working_tree_facts(facts("old\n")),
        "old\n",
        1,
    )?;
    let newer = store.annotate(
        Draft::new(
            Author::User,
            Path::new("b.md"),
            LineRange::new(1, 1),
            "newer match",
        )
        .with_working_tree_facts(facts("new\n")),
        "new\n",
        2,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options.limits.comparison_bytes = 5;
            options
        })
        .build()?;
    app.trigger_landing(Some(CommitId::parse(&head)?));
    app.settle_background();

    assert_eq!(
        app.thread(&older).and_then(|thread| thread.landed_commit()),
        None
    );
    assert_eq!(
        app.thread(&newer).and_then(|thread| thread.landed_commit()),
        Some(head.as_str())
    );
    Ok(())
}

#[test]
fn landing_hidden_board_selection_cannot_receive_lifecycle_action() -> anyhow::Result<()> {
    let dir = TempDir::new("landing-hidden-board-cursor")?;
    let root = dir.0.join("ws");
    git::init(&root)?;
    git::commit_and_stage(&root, &[("a.md", "old\n")])?;
    let workspace = Workspace::discover(&root)?;
    let old = workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("old HEAD"))?;
    fs::write(root.join("a.md"), "landed\n")?;
    let mut store = Store::open(dir.0.join("threads.jsonl"))?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "candidate",
        )
        .with_working_tree_facts(WorkingTreeFacts::new(
            Some(old.clone()),
            WorkingTreeState::Modified,
            Some(ContentIdentity::from_text("landed\n")),
            workspace.identity(),
            FullFileDigest::from_bytes(b"landed\n"),
        )),
        "landed\n",
        1,
    )?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::EmptyTree);
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&old)?));
    app.settle_background();
    app.open_review();
    assert_eq!(app.thread_cursor().thread(), Some(&id));

    git::commit_and_stage(&root, &[("a.md", "landed\n")])?;
    let landed = Workspace::discover(&root)?
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("landed HEAD"))?;
    app.trigger_landing(Some(CommitId::parse(&landed)?));
    app.settle_background();
    assert!(app.thread_cursor().thread().is_none());

    app.thread_toggle_resolved();
    assert_eq!(
        app.thread(&id)
            .map(fathomable_core::annotations::Thread::lifecycle),
        Some(Lifecycle::Active)
    );
    Ok(())
}

#[test]
fn concurrently_folded_landing_clears_hidden_board_selection() -> anyhow::Result<()> {
    let dir = TempDir::new("landing-concurrent-fold")?;
    let root = dir.0.join("ws");
    let store_path = dir.0.join("threads.jsonl");
    git::init(&root)?;
    git::commit_and_stage(&root, &[("a.md", "old\n")])?;
    let workspace = Workspace::discover(&root)?;
    let old = workspace
        .head_commit()
        .ok_or_else(|| anyhow::anyhow!("old HEAD"))?;
    fs::write(root.join("a.md"), "new\n")?;
    let mut store = Store::open(&store_path)?;
    let id = store.annotate(
        Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "candidate",
        )
        .with_working_tree_facts(WorkingTreeFacts::new(
            Some(old.clone()),
            WorkingTreeState::Modified,
            Some(ContentIdentity::from_text("new\n")),
            workspace.identity(),
            FullFileDigest::from_bytes(b"new\n"),
        )),
        "new\n",
        1,
    )?;
    let mut peer = Store::open(&store_path)?;
    let mut app = AppBuilder::at(&root)
        .unopened()
        .options(move |mut options| {
            options.store = Some(store);
            options
        })
        .build()?;
    app.set_comparison_base(ComparisonEndpoint::EmptyTree);
    app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(&old)?));
    app.settle_background();
    app.open_review();
    assert_eq!(app.thread_cursor().thread(), Some(&id));

    git::commit_and_stage(&root, &[("a.md", "new\n")])?;
    let landed = CommitId::parse(
        &Workspace::discover(&root)?
            .head_commit()
            .ok_or_else(|| anyhow::anyhow!("landed HEAD"))?,
    )?;
    let candidate = app
        .thread(&id)
        .and_then(fathomable_core::annotations::Thread::landing_candidate)
        .ok_or_else(|| anyhow::anyhow!("landing candidate"))?;
    app.trigger_landing(Some(landed.clone()));
    assert_eq!(
        peer.land(&candidate, &landed)?,
        fathomable_core::annotations::LandingOutcome::Applied
    );
    app.settle_background();

    assert_eq!(
        app.thread(&id).and_then(|thread| thread.landed_commit()),
        Some(landed.as_str())
    );
    assert!(app.thread_cursor().thread().is_none());
    app.thread_toggle_resolved();
    assert_eq!(
        app.thread(&id)
            .map(fathomable_core::annotations::Thread::lifecycle),
        Some(Lifecycle::Active)
    );
    Ok(())
}
