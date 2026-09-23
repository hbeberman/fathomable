// @okf-doc: /decisions/0093-version-scoped-viewer-membership.md
//! Bounded, ordered reconciliation of mutable origins with immutable commits.

use std::collections::{BTreeMap, VecDeque};

use fathomable_core::annotations::{LandingCandidate, ThreadId};
use fathomable_core::config::LimitsConfig;
use fathomable_core::workspace::{
    Cancellation, CheckoutIdentity, CommitId, ExactFileMatch, ExactFileProgress, HeadState,
    Workspace,
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
}

#[derive(Debug, Clone)]
struct Capture {
    checkout: CheckoutIdentity,
    commit: Option<CommitId>,
    bound: bool,
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

fn captured_commit(workspace: &Workspace) -> Result<Option<CommitId>, String> {
    if !workspace.is_git() {
        return Ok(None);
    }
    match workspace.observe_head(0).state() {
        HeadState::Symbolic { commit, .. } | HeadState::Detached { commit } => {
            Ok(Some(commit.clone()))
        }
        HeadState::Unborn => Ok(None),
        HeadState::Unavailable { error } => Err(error.clone()),
    }
}

fn bind_commits(context: &mut Context, cancellation: &Cancellation, failures: &mut Vec<String>) {
    for capture in &mut context.captures {
        if cancellation.is_cancelled() {
            break;
        }
        if capture.bound {
            continue;
        }
        capture.bound = true;
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
        match captured_commit(&workspace) {
            Ok(commit) => capture.commit = commit,
            Err(error) => failures.push(error),
        }
    }
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
    retry: Option<Request>,
}

impl Landing {
    pub(super) fn new() -> Self {
        Self {
            worker: Worker::new(reconcile),
            queued: None,
            retry: None,
        }
    }

    fn retain_later(&mut self, request: Request) {
        if self.queued.is_none() {
            self.queued = Some(request);
        } else {
            tracing::warn!("landing reconciliation coalesced a newer request");
            self.retry = Some(request);
        }
    }

    fn submit(&mut self, request: Request) {
        if self.worker.pending() {
            self.retain_later(request);
        } else if let Err(error) = self.worker.submit(request) {
            tracing::warn!(%error, "cannot start landing reconciliation");
        }
    }

    fn next_request(&mut self, chained: Option<Request>) -> Option<Request> {
        let next = chained
            .or_else(|| self.queued.take())
            .or_else(|| self.retry.take());
        if next.is_some() && self.queued.is_none() {
            self.queued = self.retry.take();
        }
        next
    }

    fn clear_retained(&mut self) {
        self.queued = None;
        self.retry = None;
    }

    pub(super) fn pending(&self) -> bool {
        self.worker.pending() || self.queued.is_some() || self.retry.is_some()
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
        let commit = match commit {
            Some(commit) => Some(commit),
            None => captured_commit(&self.workspace)?,
        };
        let mut captures = vec![Capture {
            checkout: active.clone(),
            commit,
            bound: true,
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
                    bound: false,
                });
            }
        }
        Ok(Some(Context { captures, limits }))
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

    /// Schedule one bounded reconciliation pass. The oldest queued pass and
    /// newest coalesced retry are retained while a pass runs.
    pub(super) fn trigger_landing(&mut self, commit: Option<CommitId>) {
        match self.landing_context(commit, self.workspace.limits().clone()) {
            Ok(Some(context)) => {
                if let Some(request) = self.landing_request(context, None) {
                    self.landing.submit(request);
                }
            }
            Ok(None) => {}
            Err(error) => {
                self.notice(error);
            }
        }
    }

    pub(super) fn poll_landing(&mut self) -> bool {
        let mut worker_stopped = false;
        let completed = match self.landing.worker.poll() {
            Ok(Some(result)) => Some(result),
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(%error, "landing reconciliation worker stopped");
                worker_stopped = true;
                None
            }
        };
        let mut changed = false;
        if let Some(mut result) = completed {
            let first_failure = result.failures.first().cloned();
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
            let chained = result.resume.take().or(continuation);
            if let Some(request) = self.landing.next_request(chained)
                && let Err(error) = self.landing.worker.submit(request)
            {
                tracing::warn!(%error, "cannot continue landing reconciliation");
                self.landing.clear_retained();
            }
        } else if worker_stopped
            && let Some(request) = self.landing.next_request(None)
            && let Err(error) = self.landing.worker.submit(request)
        {
            tracing::warn!(%error, "cannot restart landing reconciliation");
            self.landing.clear_retained();
        }

        if changed {
            self.trigger_thread_moves();
            self.refresh_all_marks();
            self.refresh_review_paths();
            self.reconcile_normal_thread_cursor();
        }
        changed
    }
}

#[cfg(test)]
mod tests;
