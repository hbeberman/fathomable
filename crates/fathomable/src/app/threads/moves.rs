// @okf-doc: /decisions/0087-global-comparisons-and-board-history.md
//! Viewer-local recovery of exact committed file moves.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use cap_std::{ambient_authority, fs::Dir};
use fathomable_core::annotations::{FullFileDigest, OriginVersion, Thread, ThreadId};
use fathomable_core::config::LimitsConfig;
use fathomable_core::workspace::{
    Cancellation, CheckoutIdentity, CommitId, ExactFile, ExactFileMatch, ExactFileProgress,
    Workspace,
};

use crate::app::App;
use crate::app::background::Worker;

#[derive(Debug)]
struct Candidate {
    id: ThreadId,
    path: PathBuf,
    source: CommitId,
    digest: Option<FullFileDigest>,
}

fn candidate(thread: &Thread, checkout: &CheckoutIdentity) -> Result<Option<Candidate>, String> {
    let (commit, digest) = match thread.origin_version() {
        OriginVersion::Commit { id } => (id.as_str(), None),
        OriginVersion::WorkingTree { .. }
        | OriginVersion::Index { .. }
        | OriginVersion::ReviewPoint { .. } => {
            let (owner, digest) = if let Some(facts) = thread.provenance().working_tree() {
                (facts.checkout(), facts.full_content())
            } else if let Some(facts) = thread.provenance().index() {
                (facts.checkout(), facts.full_content())
            } else {
                let Some(facts) = thread.provenance().review_point() else {
                    return Ok(None);
                };
                (facts.checkout(), facts.full_content())
            };
            if owner.repository() != checkout.repository() {
                return Ok(None);
            }
            let Some(commit) = thread
                .landed_commit()
                .or_else(|| thread.origin_version().commit_reference())
            else {
                return Ok(None);
            };
            (commit, Some(digest.clone()))
        }
        OriginVersion::EmptyTree | OriginVersion::Unknown => return Ok(None),
    };
    Ok(Some(Candidate {
        id: thread.id().clone(),
        path: thread.path().to_path_buf(),
        source: CommitId::parse(commit)
            .map_err(|error| format!("rename source commit unavailable: {error}"))?,
        digest,
    }))
}

#[derive(Debug)]
struct Request {
    checkout: CheckoutIdentity,
    head: CommitId,
    limits: LimitsConfig,
    candidates: Vec<Candidate>,
}

#[derive(Debug)]
struct ResultSet {
    checkout: CheckoutIdentity,
    head: CommitId,
    paths: HashMap<ThreadId, PathBuf>,
    failures: Vec<String>,
}

fn verified_origins(
    workspace: &mut Workspace,
    source: &CommitId,
    candidates: &[Candidate],
    moves: &HashMap<PathBuf, PathBuf>,
    checkout: &CheckoutIdentity,
    failures: &mut Vec<String>,
) -> HashSet<(PathBuf, FullFileDigest)> {
    let expectations: Vec<ExactFile> = candidates
        .iter()
        .filter(|candidate| moves.contains_key(&candidate.path))
        .filter_map(|candidate| {
            candidate.digest.as_ref().map(|digest| {
                ExactFile::new(checkout.clone(), candidate.path.clone(), digest.clone())
            })
        })
        .collect();
    let mut verified = HashSet::new();
    let mut remaining = expectations.as_slice();
    while !remaining.is_empty() {
        let batch = match workspace.match_commit_files(source, remaining) {
            Ok(batch) => batch,
            Err(error) => {
                failures.push(error.to_string());
                break;
            }
        };
        let (matches, progress) = batch.into_parts();
        for (expectation, matched) in remaining.iter().zip(&matches) {
            match matched.result() {
                ExactFileMatch::Match => {
                    verified.insert((
                        expectation.path().to_path_buf(),
                        expectation.digest().clone(),
                    ));
                }
                ExactFileMatch::Unavailable(error) => failures.push(error.clone()),
                ExactFileMatch::Missing | ExactFileMatch::Mismatch => {}
            }
        }
        match progress {
            ExactFileProgress::Complete => break,
            ExactFileProgress::ContinueAt { next } if next > 0 => {
                remaining = &remaining[next..];
            }
            ExactFileProgress::ContinueAt { .. } => {
                failures.push("rename verification made no progress".to_owned());
                break;
            }
        }
    }
    verified
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "the background worker owns the cancellation handle"
)]
fn recover(request: Request, cancellation: Cancellation) -> ResultSet {
    let mut result = ResultSet {
        checkout: request.checkout.clone(),
        head: request.head.clone(),
        paths: HashMap::new(),
        failures: Vec::new(),
    };
    let mut workspace = match Workspace::discover(request.checkout.checkout()) {
        Ok(workspace) if workspace.identity() == request.checkout => workspace,
        Ok(_) => {
            result
                .failures
                .push("rename checkout identity changed".to_owned());
            return result;
        }
        Err(error) => {
            result.failures.push(error.to_string());
            return result;
        }
    };
    workspace.set_limits(request.limits);
    workspace.set_cancellation(cancellation.clone());
    let dir = match Dir::open_ambient_dir(workspace.root(), ambient_authority()) {
        Ok(dir) => dir,
        Err(error) => {
            result
                .failures
                .push(format!("rename checkout unavailable: {error}"));
            return result;
        }
    };
    let mut groups: BTreeMap<CommitId, Vec<Candidate>> = BTreeMap::new();
    for candidate in request.candidates {
        match dir.open(&candidate.path).and_then(|file| file.metadata()) {
            Ok(metadata) if metadata.is_file() => continue,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                result
                    .failures
                    .push(format!("rename source path unavailable: {error}"));
                continue;
            }
        }
        groups
            .entry(candidate.source.clone())
            .or_default()
            .push(candidate);
    }
    if groups.len() > MAX_MOVE_SOURCES {
        result
            .failures
            .push("rename recovery limited to 64 source commits".to_owned());
        return result;
    }
    for (source, candidates) in groups {
        if cancellation.is_cancelled() {
            return result;
        }
        let moves = match workspace.exact_commit_moves(&source, &request.head) {
            Ok(moves) => moves.into_iter().collect::<HashMap<_, _>>(),
            Err(error) => {
                result.failures.push(error.to_string());
                continue;
            }
        };
        if moves.is_empty() {
            continue;
        }
        let verified = verified_origins(
            &mut workspace,
            &source,
            &candidates,
            &moves,
            &request.checkout,
            &mut result.failures,
        );
        for candidate in candidates {
            if let Some(path) = moves.get(&candidate.path)
                && candidate.digest.as_ref().is_none_or(|digest| {
                    verified.contains(&(candidate.path.clone(), digest.clone()))
                })
            {
                result.paths.insert(candidate.id, path.clone());
            }
        }
    }
    match workspace.exact_head_commit() {
        Ok(commit) if commit.id() == request.head => {}
        Ok(_) => {
            result.paths.clear();
            result
                .failures
                .push("rename checkout HEAD changed".to_owned());
        }
        Err(error) => {
            result.paths.clear();
            result.failures.push(error.to_string());
        }
    }
    result
}

#[derive(Debug)]
pub(in crate::app) struct ThreadMoves {
    worker: Worker<Request, ResultSet>,
    checkout: Option<CheckoutIdentity>,
    head: Option<CommitId>,
    pub(in crate::app) paths: HashMap<ThreadId, PathBuf>,
    renames: Vec<(PathBuf, PathBuf, bool)>,
}

// Each source requires two bounded complete-tree reads, so bound distinct
// histories independently of the configured per-tree path ceiling.
const MAX_MOVE_SOURCES: usize = 64;

impl ThreadMoves {
    pub(in crate::app) fn new() -> Self {
        Self {
            worker: Worker::new(recover),
            checkout: None,
            head: None,
            paths: HashMap::new(),
            renames: Vec::new(),
        }
    }

    pub(in crate::app) fn pending(&self) -> bool {
        self.worker.pending()
    }

    pub(in crate::app) fn clear(&mut self) {
        self.worker.cancel();
        self.checkout = None;
        self.head = None;
        self.paths.clear();
        self.renames.clear();
    }

    pub(in crate::app) fn renamed(
        &mut self,
        from: &Path,
        to: &Path,
        is_dir: bool,
        limit: usize,
    ) -> bool {
        if self.renames.len() >= limit {
            self.clear();
            return false;
        }
        self.renames
            .push((from.to_path_buf(), to.to_path_buf(), is_dir));
        true
    }

    pub(in crate::app) fn matches_workspace(&self, workspace: &Workspace) -> bool {
        self.checkout.as_ref() == Some(&workspace.identity())
            && self.head.as_ref().map(CommitId::as_str) == workspace.head_commit().as_deref()
    }
}

impl App {
    pub(in crate::app) fn trigger_thread_moves(&mut self) {
        let checkout = self.workspace.identity();
        let head = self
            .workspace
            .head_commit()
            .and_then(|head| CommitId::parse(head).ok());
        if self.thread_moves.checkout.as_ref() != Some(&checkout)
            || self.thread_moves.head.as_ref() != head.as_ref()
        {
            self.thread_moves.paths.clear();
            self.thread_moves.renames.clear();
            self.local_thread_paths.clear();
            self.refresh_all_marks();
        }
        self.thread_moves.checkout = Some(checkout.clone());
        self.thread_moves.head.clone_from(&head);
        let Some(head) = head else {
            self.thread_moves.worker.cancel();
            return;
        };
        let mut candidates = Vec::new();
        for thread in self
            .store
            .iter()
            .flat_map(fathomable_core::annotations::Store::threads)
        {
            match candidate(thread, &checkout) {
                Ok(Some(candidate)) => candidates.push(candidate),
                Ok(None) => {}
                Err(error) => {
                    self.thread_moves.worker.cancel();
                    self.notice(error);
                    return;
                }
            }
        }
        if candidates.is_empty() {
            self.thread_moves.worker.cancel();
            self.thread_moves.paths.clear();
            return;
        }
        let limits = self.workspace.limits().clone();
        if candidates.len() > limits.comparison_path_limit() {
            self.thread_moves.worker.cancel();
            self.notice("rename recovery exceeds the comparison path limit");
            return;
        }
        if let Err(error) = self.thread_moves.worker.submit(Request {
            checkout,
            head,
            limits,
            candidates,
        }) {
            self.notice(format!("cannot start rename recovery: {error}"));
        }
    }

    pub(in crate::app) fn poll_thread_moves(&mut self) -> bool {
        let result = match self.thread_moves.worker.poll() {
            Ok(Some(result)) => result,
            Ok(None) => return false,
            Err(error) => {
                self.notice(format!("rename recovery stopped: {error}"));
                return true;
            }
        };
        if Some(&result.checkout) != self.thread_moves.checkout.as_ref()
            || Some(&result.head) != self.thread_moves.head.as_ref()
            || result.checkout != self.workspace.identity()
            || self.workspace.head_commit().as_deref() != Some(result.head.as_str())
        {
            return false;
        }
        if let Some(failure) = result.failures.first() {
            self.notice(format!("rename recovery incomplete: {failure}"));
        }
        let mut paths = HashMap::new();
        let root = self.workspace.root();
        let dir = match Dir::open_ambient_dir(root, ambient_authority()) {
            Ok(dir) => dir,
            Err(error) => {
                self.notice(format!("rename recovery cannot open checkout: {error}"));
                return true;
            }
        };
        for (id, mut path) in result.paths {
            for (from, to, is_dir) in &self.thread_moves.renames {
                if path == *from {
                    path.clone_from(to);
                } else if *is_dir && let Ok(rest) = path.strip_prefix(from) {
                    path = to.join(rest);
                }
            }
            match dir.open(&path).and_then(|file| file.metadata()) {
                Ok(metadata) if metadata.is_file() => {
                    paths.insert(id, path);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    self.notice(format!(
                        "rename recovery cannot inspect destination: {error}"
                    ));
                }
            }
        }
        let changed = paths != self.thread_moves.paths;
        self.thread_moves.paths = paths;
        if changed {
            self.refresh_all_marks();
            self.reconcile_normal_thread_cursor();
        }
        changed || !result.failures.is_empty()
    }
}
