// @okf-doc: /decisions/0093-version-scoped-viewer-membership.md
//! Bounded, ordered reconciliation of mutable origins with immutable commits.

use std::collections::{BTreeMap, VecDeque};

use fathomable_core::annotations::{LandingCandidate, ThreadId};
use fathomable_core::config::LimitsConfig;
use fathomable_core::workspace::{
    Cancellation, CheckoutIdentity, CommitId, ExactFileMatch, ExactFileProgress, Workspace,
};

use super::App;
use super::background::Worker;

#[derive(Debug)]
struct Request {
    candidates: Vec<LandingCandidate>,
    context: Context,
    next_page: Option<PageCursor>,
}

#[derive(Debug, Clone)]
struct Continuation {
    after: PageCursor,
    context: Context,
}

#[derive(Debug, Clone)]
struct Context {
    captures: Vec<Capture>,
    limits: LimitsConfig,
    bound: bool,
}

#[derive(Debug, Clone)]
struct Capture {
    checkout: CheckoutIdentity,
    commit: Option<CommitId>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PageCursor {
    created: u64,
    thread: ThreadId,
}

#[derive(Debug)]
struct Match {
    candidate: LandingCandidate,
    commit: CommitId,
}

#[derive(Debug, Default)]
struct ResultSet {
    matches: Vec<Match>,
    failures: Vec<String>,
    continuation: Option<Continuation>,
    resume: Option<Request>,
}

fn bind_commits(context: &mut Context, cancellation: &Cancellation, failures: &mut Vec<String>) {
    if context.bound {
        return;
    }
    for capture in &mut context.captures {
        if cancellation.is_cancelled() {
            break;
        }
        if capture.commit.is_some() {
            continue;
        }
        let mut workspace = match Workspace::discover(capture.checkout.checkout()) {
            Ok(workspace) if workspace.identity() == capture.checkout => workspace,
            Ok(_) => {
                failures.push("landing checkout identity changed".to_owned());
                continue;
            }
            Err(error) => {
                failures.push(error.to_string());
                continue;
            }
        };
        workspace.set_limits(context.limits.clone());
        workspace.set_cancellation(cancellation.clone());
        capture.commit = workspace
            .head_commit()
            .and_then(|id| CommitId::parse(id).ok());
    }
    context.bound = true;
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "background workers own the cancellation handle by contract"
)]
fn reconcile(request: Request, cancellation: Cancellation) -> ResultSet {
    let Request {
        candidates,
        mut context,
        next_page,
    } = request;
    let mut result = ResultSet::default();
    bind_commits(&mut context, &cancellation, &mut result.failures);
    let mut groups: VecDeque<(CheckoutIdentity, Vec<LandingCandidate>)> = VecDeque::new();
    for candidate in candidates {
        if let Some((_, candidates)) = groups
            .iter_mut()
            .find(|(checkout, _)| checkout == candidate.checkout())
        {
            candidates.push(candidate);
        } else {
            groups.push_back((candidate.checkout().clone(), vec![candidate]));
        }
    }
    while let Some((identity, candidates)) = groups.pop_front() {
        if cancellation.is_cancelled() {
            break;
        }
        let mut workspace = match Workspace::discover(identity.checkout()) {
            Ok(workspace) if workspace.identity() == identity => workspace,
            Ok(_) => {
                result
                    .failures
                    .push("landing checkout identity changed".to_owned());
                continue;
            }
            Err(error) => {
                result.failures.push(error.to_string());
                continue;
            }
        };
        workspace.set_limits(context.limits.clone());
        workspace.set_cancellation(cancellation.clone());
        let commit = context
            .captures
            .iter()
            .find(|capture| capture.checkout == identity)
            .and_then(|capture| capture.commit.clone());
        let Some(commit) = commit else {
            continue;
        };
        let exact = candidates
            .iter()
            .map(LandingCandidate::exact_file)
            .collect::<Vec<_>>();
        match workspace.match_commit_files(&commit, &exact) {
            Ok(batch) => {
                let (matches, progress) = batch.into_parts();
                for (candidate, matched) in candidates.iter().zip(matches) {
                    match matched.result() {
                        ExactFileMatch::Match => result.matches.push(Match {
                            candidate: candidate.clone(),
                            commit: commit.clone(),
                        }),
                        ExactFileMatch::Unavailable(error) => {
                            result.failures.push(error.clone());
                        }
                        ExactFileMatch::Mismatch | ExactFileMatch::Missing => {}
                    }
                }
                if let ExactFileProgress::ContinueAt { next } = progress {
                    let mut remaining = candidates.into_iter().skip(next).collect::<Vec<_>>();
                    remaining.extend(
                        groups
                            .into_iter()
                            .flat_map(|(_, candidates)| candidates.into_iter()),
                    );
                    result.resume = Some(Request {
                        candidates: remaining,
                        context,
                        next_page,
                    });
                    return result;
                }
            }
            Err(error) => result.failures.push(error.to_string()),
        }
    }
    result.continuation = next_page.map(|after| Continuation { after, context });
    result
}

#[derive(Debug)]
pub(super) struct Landing {
    worker: Worker<Request, ResultSet>,
    queued: Option<Request>,
    retry_needed: bool,
}

impl Landing {
    pub(super) fn new() -> Self {
        Self {
            worker: Worker::new(reconcile),
            queued: None,
            retry_needed: false,
        }
    }

    fn submit(&mut self, request: Request) {
        if self.worker.pending() {
            if self.queued.is_none() {
                self.queued = Some(request);
            } else {
                tracing::warn!("landing reconciliation already has a pending request");
                self.retry_needed = true;
            }
        } else if let Err(error) = self.worker.submit(request) {
            tracing::warn!(%error, "cannot start landing reconciliation");
            self.retry_needed = true;
        }
    }

    pub(super) fn pending(&self) -> bool {
        self.worker.pending() || self.queued.is_some()
    }
}

impl App {
    fn landing_context(
        &self,
        commit: Option<CommitId>,
        limits: LimitsConfig,
    ) -> Result<Option<Context>, String> {
        let Some(store) = self.store.as_ref() else {
            return Ok(None);
        };
        let active = self.workspace.identity();
        let limit = limits.comparison_path_limit();
        if limit == 0 {
            return Ok(None);
        }
        let mut captures = vec![Capture {
            checkout: active.clone(),
            commit,
        }];
        for thread in store.all_threads() {
            let Some(candidate) = thread.landing_candidate() else {
                continue;
            };
            if candidate.checkout().repository() != active.repository()
                || (candidate.checkout() != &active
                    && !self
                        .worktrees
                        .iter()
                        .any(|worktree| worktree.root() == candidate.checkout().checkout()))
            {
                continue;
            }
            if !captures
                .iter()
                .any(|capture| &capture.checkout == candidate.checkout())
            {
                if captures.len() >= limits.discovery_entries {
                    return Err(format!(
                        "landing checkout capture exceeds the {}-entry discovery limit",
                        limits.discovery_entries
                    ));
                }
                captures.push(Capture {
                    checkout: candidate.checkout().clone(),
                    commit: None,
                });
            }
        }
        Ok(Some(Context {
            captures,
            limits,
            bound: false,
        }))
    }

    fn landing_request(&self, context: Context, after: Option<&PageCursor>) -> Option<Request> {
        let store = self.store.as_ref()?;
        let limit = context.limits.comparison_path_limit();
        let mut page = BTreeMap::new();
        for thread in store.all_threads() {
            let cursor = PageCursor {
                created: thread.created(),
                thread: thread.id().clone(),
            };
            if after.is_some_and(|after| &cursor <= after) {
                continue;
            }
            let Some(candidate) = thread.landing_candidate() else {
                continue;
            };
            if !context
                .captures
                .iter()
                .any(|capture| &capture.checkout == candidate.checkout())
            {
                continue;
            }
            page.insert(cursor, candidate);
            if page.len() > limit.saturating_add(1) {
                page.pop_last();
            }
        }
        let has_more = page.len() > limit;
        if has_more {
            page.pop_last();
        }
        let next_page = has_more
            .then(|| page.last_key_value().map(|(cursor, _)| cursor.clone()))
            .flatten();
        let candidates = page.into_values().collect::<Vec<_>>();
        (!candidates.is_empty()).then_some(Request {
            candidates,
            context,
            next_page,
        })
    }

    /// Schedule one bounded reconciliation pass. Running and queued passes are
    /// never superseded. At most one later pass is retained while a pass runs.
    pub(super) fn trigger_landing(&mut self, commit: Option<CommitId>) {
        self.landing.retry_needed = false;
        match self.landing_context(commit, self.workspace.limits().clone()) {
            Ok(Some(context)) => {
                if let Some(request) = self.landing_request(context, None) {
                    self.landing.submit(request);
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.notice(error);
                self.landing.retry_needed = true;
            }
        }
    }

    pub(super) fn poll_landing(&mut self) -> bool {
        let completed = match self.landing.worker.poll() {
            Ok(Some(result)) => Some(result),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, "landing reconciliation worker stopped");
                self.landing.retry_needed = true;
                None
            }
        };
        let mut changed = false;
        if let Some(mut result) = completed {
            let first_failure = result.failures.first().cloned();
            self.landing.retry_needed |= !result.failures.is_empty();
            if let Some(store) = self.store.as_mut() {
                for matched in result.matches {
                    // `land` reloads under its write lock, even when the
                    // candidate is already landed, stale, or conflicting.
                    let before = store.activity_cursor();
                    let outcome = store.land(&matched.candidate, &matched.commit);
                    changed |= store.activity_cursor() != before;
                    match outcome {
                        Ok(fathomable_core::annotations::LandingOutcome::Applied) => changed = true,
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(%error, "cannot persist annotation landing");
                            self.landing.retry_needed = true;
                        }
                    }
                }
            }
            if let Some(failure) = first_failure {
                self.notice(format!("landing reconciliation incomplete: {failure}"));
            }
            let continuation = result.continuation.take().and_then(|continuation| {
                self.landing_request(continuation.context, Some(&continuation.after))
            });
            if let Some(request) = result
                .resume
                .take()
                .or(continuation)
                .or_else(|| self.landing.queued.take())
                && let Err(error) = self.landing.worker.submit(request)
            {
                tracing::warn!(%error, "cannot continue landing reconciliation");
                self.landing.retry_needed = true;
            }
        }

        if changed {
            self.refresh_all_marks();
            self.refresh_review_paths();
            self.reconcile_normal_thread_cursor();
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use fathomable_core::annotations::{
        Author, ContentIdentity, Draft, FullFileDigest, Lifecycle, LineRange, Store, ThreadId,
        WorkingTreeFacts, WorkingTreeState,
    };
    use fathomable_core::workspace::{CommitId, ComparisonEndpoint, Workspace};
    use fathomable_testing::{TempDir, git};

    use super::{App, Cancellation, ResultSet, reconcile};
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
    fn path_page_keeps_non_active_checkout_commit_across_head_and_root_changes()
    -> anyhow::Result<()> {
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
}
