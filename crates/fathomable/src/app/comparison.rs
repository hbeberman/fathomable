// @okf-doc: /decisions/0087-global-comparisons-and-board-history.md
//! Viewer-wide comparison selection and explicit review-point endpoints.
//!
//! The app owns one comparison for the active checkout.  File views only
//! render the projection of that selection; they do not select their own
//! pair of endpoints.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fathomable_core::config::DiffMode;
use fathomable_core::diff::{Compare, Comparison, PathChangeKind, PathState};
use fathomable_core::review_points::{ReviewPoint, ReviewPointStore};
use fathomable_core::status::{Changes, Entry, State as GitState, Status};
use fathomable_core::workspace::{
    Commit, CommitId, ComparisonEndpoint, RevisionChoiceKind, Workspace,
};
use serde::{Deserialize, Serialize};

use super::App;
use super::background::Worker;
use super::diff::{DiffBody, Text};

const PREFERENCE_FILE: &str = "comparison.json";
const COMMIT_PICKER_LIMIT: usize = 500;

#[derive(Debug)]
struct Request {
    root: PathBuf,
    base: ComparisonEndpoint,
    target: ComparisonEndpoint,
    review_points: Option<PathBuf>,
    limits: fathomable_core::config::LimitsConfig,
    compare: Compare,
}

#[derive(Debug)]
struct Computed {
    comparison: Comparison,
    counts: HashMap<PathBuf, (usize, usize)>,
}

#[derive(Debug)]
struct TargetRequest {
    root: PathBuf,
    endpoint: ComparisonEndpoint,
    limits: fathomable_core::config::LimitsConfig,
}

fn target_paths(
    request: TargetRequest,
    cancellation: fathomable_core::workspace::Cancellation,
) -> Result<Vec<PathBuf>, String> {
    let mut workspace = Workspace::discover(request.root).map_err(|error| error.to_string())?;
    workspace.set_limits(request.limits);
    workspace.set_cancellation(cancellation);
    workspace
        .endpoint_paths(&request.endpoint)
        .map_err(|error| error.to_string())
}

fn compute(
    request: Request,
    cancellation: fathomable_core::workspace::Cancellation,
) -> Result<Computed, String> {
    let mut workspace = Workspace::discover(&request.root).map_err(|error| error.to_string())?;
    workspace.set_limits(request.limits);
    workspace.set_cancellation(cancellation);
    let store = request
        .review_points
        .map(ReviewPointStore::open)
        .transpose()
        .map_err(|error| error.to_string())?;
    let comparison = match (&request.base, &request.target) {
        (ComparisonEndpoint::ReviewPoint(id), ComparisonEndpoint::WorkingTree) => {
            let store = store
                .as_ref()
                .ok_or_else(|| format!("review point {id} is unavailable"))?;
            let point = store
                .get(id)
                .ok_or_else(|| format!("review point {id} is unavailable"))?;
            store
                .compare_to_working(point, &mut workspace)
                .map_err(|error| error.to_string())?
        }
        (ComparisonEndpoint::ReviewPoint(_), _) | (_, ComparisonEndpoint::ReviewPoint(_)) => {
            return Err("review points compare only against the working tree".to_owned());
        }
        _ => workspace
            .compare(request.base.clone(), request.target.clone())
            .map_err(|error| error.to_string())?,
    };
    let mut counts = HashMap::new();
    for change in comparison.changes() {
        if change.kind() == PathChangeKind::Missing {
            let detail = match (change.base(), change.target()) {
                (PathState::Missing(detail), _) | (_, PathState::Missing(detail)) => {
                    detail.as_str()
                }
                _ => "comparison content unavailable",
            };
            return Err(format!("{}: {detail}", change.path().display()));
        }
        if matches!(
            change.kind(),
            PathChangeKind::Binary | PathChangeKind::Unsupported
        ) {
            continue;
        }
        let text = |endpoint: &ComparisonEndpoint| -> Result<Option<String>, String> {
            if let ComparisonEndpoint::ReviewPoint(id) = endpoint {
                let store = store
                    .as_ref()
                    .ok_or_else(|| format!("review point {id} is unavailable"))?;
                let point = store
                    .get(id)
                    .ok_or_else(|| format!("review point {id} is unavailable"))?;
                store
                    .load_bytes(point, &workspace, change.path())
                    .map_err(|error| error.to_string())?
                    .map_or(Ok(None), |bytes| Ok(String::from_utf8(bytes).ok()))
            } else {
                workspace
                    .endpoint_bytes(endpoint, change.path())
                    .map_err(|error| error.to_string())
                    .map(|bytes| bytes.and_then(|bytes| String::from_utf8(bytes).ok()))
            }
        };
        let base = text(&request.base)?;
        let target = text(&request.target)?;
        let count = match (base, target) {
            (Some(base), Some(target)) => {
                fathomable_core::diff::Diff::compare(&base, &target, request.compare.whitespace)
                    .counts()
            }
            (None, Some(target)) => (target.lines().count(), 0),
            (Some(base), None) => (0, base.lines().count()),
            (None, None) => (0, 0),
        };
        counts.insert(change.path().to_path_buf(), count);
    }
    Ok(Computed { comparison, counts })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "name")]
pub(crate) enum EndpointAlias {
    Head,
    HeadParent,
    Tag(String),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EndpointRoles {
    pub(crate) base: bool,
    pub(crate) target: bool,
}

/// One app-owned comparison selection and its last successful result.
#[derive(Debug)]
pub(crate) struct State {
    dirs: fathomable_core::XdgDirs,
    base: ComparisonEndpoint,
    target: ComparisonEndpoint,
    base_alias: Option<EndpointAlias>,
    target_alias: Option<EndpointAlias>,
    compare: Compare,
    preference: PathBuf,
    persisted: bool,
    #[cfg(test)]
    refresh_count: u64,
    observed_head: Option<String>,
    current: Option<Comparison>,
    error: Option<String>,
    worker: Worker<Request, Result<Computed, String>>,
    counts: HashMap<PathBuf, (usize, usize)>,
    annotation_frozen: bool,
    refresh_deferred: bool,
    pub(super) restore_mode: Option<DiffMode>,
    target_worker: Worker<TargetRequest, Result<Vec<PathBuf>, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Preference {
    base: String,
    target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_alias: Option<EndpointAlias>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_alias: Option<EndpointAlias>,
    whitespace: bool,
}

impl State {
    /// Create the default selection, preferring a persisted checkout choice.
    pub(crate) fn load(
        dirs: &fathomable_core::XdgDirs,
        workspace: &Workspace,
        compare: Compare,
    ) -> Self {
        let preference_dir = dirs.comparison_dir(workspace.root());
        let preference = preference_dir.join(PREFERENCE_FILE);
        let (default_base, default_alias) = head_endpoint(workspace);
        let mut state = Self {
            dirs: dirs.clone(),
            base: default_base,
            target: ComparisonEndpoint::WorkingTree,
            base_alias: default_alias,
            target_alias: None,
            compare,
            preference,
            persisted: false,
            #[cfg(test)]
            refresh_count: 0,
            observed_head: workspace.head_commit(),
            current: None,
            error: None,
            worker: Worker::new(compute),
            counts: HashMap::new(),
            annotation_frozen: false,
            refresh_deferred: false,
            restore_mode: None,
            target_worker: Worker::new(target_paths),
        };
        if let Err(error) = dirs.prepare_state_dir(&preference_dir) {
            state.error = Some(format!("comparison preference unavailable: {error}"));
            tracing::warn!(%error, "cannot open comparison preference directory");
            return state;
        }
        let bytes = match fathomable_core::private_state::read(&state.preference) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => {
                state.error = Some(format!("comparison preference unavailable: {error}"));
                tracing::warn!(%error, "cannot read comparison preference");
                None
            }
        };
        if let Some(bytes) = bytes
            && let Ok(saved) = serde_json::from_slice::<Preference>(&bytes)
        {
            state.persisted = true;
            if let Some(base) = parse_endpoint(&saved.base) {
                state.base = base;
            }
            if let Some(target) = parse_endpoint(&saved.target) {
                state.target = target;
            }
            state.base_alias = saved.base_alias;
            state.target_alias = saved.target_alias;
            state.compare.whitespace = if saved.whitespace {
                fathomable_core::diff::Whitespace::Ignore
            } else {
                fathomable_core::diff::Whitespace::Exact
            };
        }
        if state.validate_aliases(workspace) {
            state.persisted = false;
        }
        state
    }

    /// The selected base endpoint.
    pub(crate) fn base(&self) -> &ComparisonEndpoint {
        &self.base
    }

    /// The selected target endpoint.
    pub(crate) fn target(&self) -> &ComparisonEndpoint {
        &self.target
    }

    pub(crate) fn base_alias(&self) -> Option<&EndpointAlias> {
        self.base_alias.as_ref()
    }

    pub(crate) fn target_alias(&self) -> Option<&EndpointAlias> {
        self.target_alias.as_ref()
    }

    /// The selected review-point Base, when there is one.
    pub(crate) fn review_point_base(&self) -> Option<&str> {
        match &self.base {
            ComparisonEndpoint::ReviewPoint(id) => Some(id),
            _ => None,
        }
    }

    pub(crate) fn observed_head(&self) -> Option<&str> {
        self.observed_head.as_deref()
    }

    /// The current line-diff settings.
    pub(crate) const fn compare(&self) -> Compare {
        self.compare
    }

    /// The last successfully computed effective comparison.
    pub(crate) fn current(&self) -> Option<&Comparison> {
        self.current
            .as_ref()
            .filter(|current| current.base() == &self.base && current.target() == &self.target)
    }

    /// A visible error from the newest refresh, if any.
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether the displayed comparison is from an older successful refresh.
    pub(crate) fn stale(&self) -> bool {
        self.error.is_some() && self.current.is_some()
    }

    /// Record a refresh failure while retaining the last successful result.
    pub(crate) fn record_error(&mut self, error: String) {
        self.cancel();
        self.error = Some(error);
    }

    /// Number of refresh attempts observed by regression tests.
    #[cfg(test)]
    pub(crate) const fn refresh_count(&self) -> u64 {
        self.refresh_count
    }

    /// Refresh mutable endpoints without discarding the last good result.
    #[cfg(not(test))]
    pub(crate) fn refresh(
        &mut self,
        workspace: &mut Workspace,
        review_points: Option<&ReviewPointStore>,
    ) {
        self.refresh_inner(workspace, review_points);
    }

    /// Refresh mutable endpoints and count the test-observed attempt.
    #[cfg(test)]
    pub(crate) fn refresh(
        &mut self,
        workspace: &mut Workspace,
        review_points: Option<&ReviewPointStore>,
    ) {
        self.refresh_count = self.refresh_count.wrapping_add(1);
        self.refresh_inner(workspace, review_points);
    }

    fn refresh_inner(
        &mut self,
        workspace: &mut Workspace,
        review_points: Option<&ReviewPointStore>,
    ) {
        if self.defer_refresh_for_annotation() {
            return;
        }
        self.target_worker.cancel();
        let aliases_changed = self.validate_aliases(workspace);
        self.error = Some("comparison scanning; coverage is incomplete".to_owned());
        if let Err(error) = self.worker.submit(Request {
            root: workspace.root().to_path_buf(),
            base: self.base.clone(),
            target: self.target.clone(),
            review_points: review_points.map(|store| store.dir().to_path_buf()),
            limits: workspace.limits().clone(),
            compare: self.compare,
        }) {
            self.error = Some(format!("cannot start comparison: {error}"));
        }
        if aliases_changed || !self.persisted {
            self.persist();
        }
        self.observed_head = workspace.head_commit();
    }

    pub(super) fn pending(&self) -> bool {
        self.worker.pending() || self.target_worker.pending()
    }

    pub(super) fn cancel(&mut self) {
        self.worker.cancel();
        self.target_worker.cancel();
        self.restore_mode = None;
    }

    pub(super) fn freeze_for_annotation(&mut self) {
        self.refresh_deferred |= self.pending();
        self.annotation_frozen = true;
        self.cancel();
    }

    pub(super) fn unfreeze_after_annotation(&mut self) -> bool {
        self.annotation_frozen = false;
        std::mem::take(&mut self.refresh_deferred)
    }

    pub(super) fn defer_refresh_for_annotation(&mut self) -> bool {
        if self.annotation_frozen {
            self.refresh_deferred = true;
            return true;
        }
        false
    }

    pub(super) fn refresh_paths(&mut self, workspace: &Workspace) {
        if self.defer_refresh_for_annotation() {
            return;
        }
        self.cancel();
        self.error = Some("Target paths scanning; coverage is incomplete".to_owned());
        if let Err(error) = self.target_worker.submit(TargetRequest {
            root: workspace.root().to_path_buf(),
            endpoint: self.target.clone(),
            limits: workspace.limits().clone(),
        }) {
            self.error = Some(format!("cannot start Target discovery: {error}"));
        }
    }

    pub(super) fn poll_paths(&mut self) -> Option<Result<Vec<PathBuf>, String>> {
        match self.target_worker.poll() {
            Ok(Some(result)) => {
                self.error = result.as_ref().err().cloned();
                Some(result)
            }
            Ok(None) => None,
            Err(error) => {
                let error = error.to_string();
                self.error = Some(error.clone());
                Some(Err(error))
            }
        }
    }

    pub(super) fn reload(&mut self, dirs: &fathomable_core::XdgDirs, workspace: &Workspace) {
        self.cancel();
        let mut next = Self::load(dirs, workspace, self.compare);
        std::mem::swap(&mut next.worker, &mut self.worker);
        std::mem::swap(&mut next.target_worker, &mut self.target_worker);
        *self = next;
    }

    pub(super) fn poll(&mut self) -> bool {
        match self.worker.poll() {
            Ok(Some(Ok(result))) => {
                self.current = Some(result.comparison);
                self.counts = result.counts;
                self.error = None;
                true
            }
            Ok(Some(Err(error))) => {
                self.error = Some(error);
                true
            }
            Err(error) => {
                self.error = Some(error.to_string());
                true
            }
            Ok(None) => false,
        }
    }

    pub(super) const fn has_preference(&self) -> bool {
        self.persisted
    }

    pub(crate) fn set_base_aliased(
        &mut self,
        base: ComparisonEndpoint,
        alias: Option<EndpointAlias>,
        workspace: &mut Workspace,
        review_points: Option<&ReviewPointStore>,
    ) {
        self.base = base;
        self.base_alias = alias;
        self.refresh(workspace, review_points);
        self.persist();
    }

    pub(crate) fn set_target_aliased(
        &mut self,
        target: ComparisonEndpoint,
        alias: Option<EndpointAlias>,
        workspace: &mut Workspace,
        review_points: Option<&ReviewPointStore>,
    ) {
        self.target = target;
        self.target_alias = alias;
        self.refresh(workspace, review_points);
        self.persist();
    }

    /// Select both endpoints and refresh their comparison once.
    pub(crate) fn set_endpoints_aliased(
        &mut self,
        base: ComparisonEndpoint,
        base_alias: Option<EndpointAlias>,
        target: ComparisonEndpoint,
        target_alias: Option<EndpointAlias>,
        workspace: &mut Workspace,
        review_points: Option<&ReviewPointStore>,
    ) -> bool {
        self.base = base;
        self.base_alias = base_alias;
        self.target = target;
        self.target_alias = target_alias;
        self.persisted = false;
        self.refresh(workspace, review_points);
        self.persisted || self.persist_result()
    }

    /// Retain a Target without evaluating it against Base.
    pub(crate) fn select_target_aliased(
        &mut self,
        target: ComparisonEndpoint,
        alias: Option<EndpointAlias>,
    ) {
        self.target = target;
        self.target_alias = alias;
        self.error = None;
        self.persist();
    }

    /// Toggle exact versus whitespace-insensitive line comparison.
    pub(crate) fn toggle_whitespace(&mut self) {
        self.compare.whitespace = match self.compare.whitespace {
            fathomable_core::diff::Whitespace::Exact => fathomable_core::diff::Whitespace::Ignore,
            fathomable_core::diff::Whitespace::Ignore => fathomable_core::diff::Whitespace::Exact,
        };
        self.persist();
    }

    /// Persist this viewer's preference outside the repository.
    pub(crate) fn persist(&mut self) {
        let _ = self.persist_result();
    }

    /// Persist this viewer's preference and report whether it reached disk.
    fn persist_result(&mut self) -> bool {
        self.persisted = false;
        let preference = Preference {
            base: endpoint_string(&self.base),
            target: endpoint_string(&self.target),
            base_alias: self.base_alias.clone(),
            target_alias: self.target_alias.clone(),
            whitespace: matches!(
                self.compare.whitespace,
                fathomable_core::diff::Whitespace::Ignore
            ),
        };
        let Ok(bytes) = serde_json::to_vec_pretty(&preference) else {
            return false;
        };
        if let Some(parent) = self.preference.parent()
            && let Err(error) = self.dirs.prepare_state_dir(parent)
        {
            tracing::warn!(%error, path = %parent.display(), "cannot create comparison preference directory");
            return false;
        }

        let temporary = self
            .preference
            .with_extension(format!("{}.tmp", std::process::id()));
        match write_atomic(&temporary, &self.preference, &bytes) {
            Ok(()) => {
                self.persisted = true;
                true
            }
            Err(error) => {
                tracing::warn!(%error, path = %self.preference.display(), "cannot persist comparison preference");
                false
            }
        }
    }

    /// Replace a missing review-point Base without changing Target or mode.
    pub(crate) fn replace_missing_review_point_base(&mut self, workspace: &Workspace) {
        let (base, alias) = head_endpoint(workspace);
        self.base = base;
        self.base_alias = alias;
        self.error = None;
        self.persist();
    }

    /// Whether the observed branch/HEAD changed since the last refresh.
    pub(crate) fn head_changed(&self, workspace: &Workspace) -> bool {
        self.target == ComparisonEndpoint::WorkingTree
            && self.observed_head.as_deref() != workspace.head_commit().as_deref()
    }

    fn validate_aliases(&mut self, workspace: &Workspace) -> bool {
        let mut changed = false;
        if self
            .base_alias
            .as_ref()
            .is_some_and(|alias| !alias_matches(alias, &self.base, workspace))
        {
            self.base_alias = None;
            changed = true;
        }
        self.validate_target_alias(workspace) || changed
    }

    fn validate_target_alias(&mut self, workspace: &Workspace) -> bool {
        if self
            .target_alias
            .as_ref()
            .is_some_and(|alias| !alias_matches(alias, &self.target, workspace))
        {
            self.target_alias = None;
            return true;
        }
        false
    }

    /// Drop a moved Target alias without resolving or evaluating Base.
    pub(crate) fn refresh_target_alias(&mut self, workspace: &Workspace) {
        if self.validate_target_alias(workspace) {
            self.persist();
        }
    }
}

fn write_atomic(temporary: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    match fathomable_core::private_state::open_read(target) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = fathomable_core::private_state::create_new(temporary)?;
    let result = file
        .write_all(bytes)
        .and_then(|()| fs::rename(temporary, target));
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn endpoint_string(endpoint: &ComparisonEndpoint) -> String {
    match endpoint {
        ComparisonEndpoint::EmptyTree => "empty-tree".to_owned(),
        ComparisonEndpoint::Commit(id) => id.to_string(),
        ComparisonEndpoint::ReviewPoint(id) => format!("review-point:{id}"),
        ComparisonEndpoint::Index => "index".to_owned(),
        ComparisonEndpoint::WorkingTree => "working-tree".to_owned(),
    }
}

fn parse_endpoint(value: &str) -> Option<ComparisonEndpoint> {
    match value {
        "empty-tree" => Some(ComparisonEndpoint::EmptyTree),
        "index" => Some(ComparisonEndpoint::Index),
        "working-tree" => Some(ComparisonEndpoint::WorkingTree),
        value if value.starts_with("review-point:") => Some(ComparisonEndpoint::ReviewPoint(
            value.strip_prefix("review-point:")?.to_owned(),
        )),
        value => CommitId::parse(value).ok().map(ComparisonEndpoint::Commit),
    }
}

fn head_endpoint(workspace: &Workspace) -> (ComparisonEndpoint, Option<EndpointAlias>) {
    workspace
        .head_commit()
        .and_then(|head| CommitId::parse(head).ok())
        .map_or((ComparisonEndpoint::EmptyTree, None), |id| {
            (ComparisonEndpoint::Commit(id), Some(EndpointAlias::Head))
        })
}

fn alias_matches(
    alias: &EndpointAlias,
    endpoint: &ComparisonEndpoint,
    workspace: &Workspace,
) -> bool {
    let ComparisonEndpoint::Commit(expected) = endpoint else {
        return false;
    };
    let revision = match alias {
        EndpointAlias::Head => "HEAD".to_owned(),
        EndpointAlias::HeadParent => "HEAD~1".to_owned(),
        EndpointAlias::Tag(name) => format!("refs/tags/{name}"),
    };
    workspace
        .resolve_revision(revision)
        .is_ok_and(|commit| commit.id() == *expected)
}

fn commit_picker_row(commit: &Commit) -> String {
    let timestamp = super::draw::format_time(commit.time());
    let date = timestamp.get(..10).unwrap_or(&timestamp);
    format!("{} {} {date}", commit.hex(), commit.subject())
}

pub(crate) fn commit_id_from_row(row: &str) -> Option<&str> {
    let id = row.split_whitespace().next()?;
    (id.len() == 40 && id.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(id)
}

fn picker_item_matches(
    item: &str,
    endpoint: &ComparisonEndpoint,
    alias: Option<&EndpointAlias>,
    observed_head: Option<&str>,
) -> bool {
    match endpoint {
        ComparisonEndpoint::WorkingTree => item == "Working tree",
        ComparisonEndpoint::Index => item == "Index",
        ComparisonEndpoint::EmptyTree => item == "Empty tree",
        ComparisonEndpoint::ReviewPoint(id) => item
            .strip_prefix("review point ")
            .and_then(|item| item.split_whitespace().next())
            .is_some_and(|item| item == id),
        ComparisonEndpoint::Commit(id) => {
            commit_id_from_row(item).is_some_and(|item| item == id.as_str())
                || (item == "HEAD" && observed_head == Some(id.as_str()))
                || matches!(
                    (item.strip_prefix("tag "), alias),
                    (Some(item), Some(EndpointAlias::Tag(name))) if item == name
                )
        }
    }
}

fn branch_name_from_row(row: &str) -> Option<&str> {
    let row = row
        .strip_prefix("remote branch ")
        .or_else(|| row.strip_prefix("branch "))?;
    Some(row.rsplit_once(" (").map_or(row, |(name, _)| name))
}

impl App {
    /// The effective comparison result, if the last refresh succeeded.
    pub(crate) fn comparison(&self) -> Option<&Comparison> {
        self.comparison.current()
    }

    /// Whether displayed comparison content matches the selected endpoints.
    pub(in crate::app) fn annotation_projection_matches_selection(&self) -> bool {
        if self.comparison.restore_mode.is_some() {
            return false;
        }
        let Some(doc) = self.current.and_then(|index| self.docs.get(index)) else {
            return false;
        };
        if self.diff_mode == DiffMode::Off {
            return doc.comparison_notice.is_none()
                && (self.comparison.target() == &ComparisonEndpoint::WorkingTree
                    || (!self.comparison.target_worker.pending()
                        && self.comparison.error().is_none()));
        }
        self.comparison.current().is_some()
            && (self.plain_default_comparison() || doc.view.comparison_projection_ready())
    }

    /// Whether the content currently projected follows the working tree.
    pub(crate) fn displayed_target_is_working_tree(&self) -> bool {
        if self.diff_mode == DiffMode::Off {
            self.comparison.target() == &ComparisonEndpoint::WorkingTree
        } else {
            self.comparison
                .current()
                .is_some_and(|comparison| comparison.target() == &ComparisonEndpoint::WorkingTree)
        }
    }

    /// The current comparison preference label.
    pub(crate) fn comparison_label(&self) -> String {
        let state = &self.comparison;
        let mut label = format!("Compare: {} → {}", state.base(), state.target());
        if state.stale() {
            label.push_str(" · stale");
        }
        label
    }

    /// Compact endpoint names for the persistent menu bar.
    pub(crate) fn comparison_menu_pair(&self) -> (String, String) {
        (
            self.comparison_menu_endpoint(self.comparison.base(), self.comparison.base_alias()),
            self.comparison_menu_endpoint(self.comparison.target(), self.comparison.target_alias()),
        )
    }

    /// Compact selected Target name for Target-only surfaces.
    pub(crate) fn comparison_target_label(&self) -> String {
        self.comparison_menu_endpoint(self.comparison.target(), self.comparison.target_alias())
    }

    /// Whether one picker row denotes the active base or target.
    pub(crate) fn picker_endpoint_roles(&self, item: &str) -> EndpointRoles {
        EndpointRoles {
            base: picker_item_matches(
                item,
                self.comparison.base(),
                self.comparison.base_alias(),
                self.comparison.observed_head(),
            ),
            target: picker_item_matches(
                item,
                self.comparison.target(),
                self.comparison.target_alias(),
                self.comparison.observed_head(),
            ),
        }
    }

    fn comparison_menu_endpoint(
        &self,
        endpoint: &ComparisonEndpoint,
        alias: Option<&EndpointAlias>,
    ) -> String {
        match (endpoint, alias) {
            (ComparisonEndpoint::Commit(_), Some(EndpointAlias::Head)) => "HEAD".to_owned(),
            (ComparisonEndpoint::Commit(_), Some(EndpointAlias::HeadParent)) => "HEAD~1".to_owned(),
            (ComparisonEndpoint::Commit(_), Some(EndpointAlias::Tag(name))) => {
                format!("Tag {name}")
            }
            (ComparisonEndpoint::Commit(id), _) => id.short().to_owned(),
            (ComparisonEndpoint::WorkingTree, _) => "WorkingTree".to_owned(),
            (ComparisonEndpoint::Index, _) => "Index".to_owned(),
            (ComparisonEndpoint::EmptyTree, _) => "EmptyTree".to_owned(),
            (ComparisonEndpoint::ReviewPoint(id), _) => self
                .review_points
                .as_ref()
                .and_then(|store| store.get(id))
                .and_then(ReviewPoint::name)
                .map_or_else(
                    || format!("Point {}", id.chars().take(8).collect::<String>()),
                    |name| format!("Point {name}"),
                ),
        }
    }

    /// Compact comparison provenance for the status line.
    pub(crate) fn comparison_badge(&self) -> String {
        let mut badge = if self.diff_mode == DiffMode::Off {
            format!("OFF Target {}", self.comparison_target_label())
        } else {
            format!(
                "CMP {} → {}",
                self.comparison.base(),
                self.comparison.target()
            )
        };
        if self.comparison.pending() {
            badge.push_str(" · scanning");
        } else if self
            .comparison
            .error()
            .is_some_and(|error| error.contains("limited"))
        {
            badge.push_str(" · limited");
        } else if self.comparison.stale() {
            badge.push_str(" · stale");
        } else if self.comparison.error().is_some() {
            badge.push_str(" · error");
        }
        if self.tree_issue.is_some()
            || self
                .tree
                .as_ref()
                .is_some_and(fathomable_core::tree::Tree::discovery_limited)
        {
            badge.push_str(" · files partial");
        }
        match &self.watch_status {
            super::watch::WatchStatus::Scanning { generation, .. } if *generation != 0 => {
                badge.push_str(" · watch scanning");
            }
            super::watch::WatchStatus::Limited { .. } => badge.push_str(" · watch limited"),
            super::watch::WatchStatus::Errored { .. } => badge.push_str(" · watch error"),
            _ => {}
        }
        badge
    }

    /// Save the selected comparison preference after a state change.
    pub(crate) fn persist_comparison(&mut self) {
        self.comparison.persist();
    }

    /// The top-level endpoint picker list.
    pub(crate) fn comparison_choices(&mut self, target: bool) -> Vec<String> {
        let mut choices = vec!["Working tree".to_owned()];
        if self.workspace.is_git() {
            choices.push("Index".to_owned());
            if self.workspace.head_commit().is_some() {
                choices.push("HEAD".to_owned());
            }
            choices.push("Tags...".to_owned());
            choices.push("Branches...".to_owned());
        }
        if !target && self.review_points.is_some() {
            choices.push("Review points...".to_owned());
        }
        choices.push("Advanced...".to_owned());
        if self.workspace.head_commit().is_some() {
            match self.workspace.recent_commits(0, COMMIT_PICKER_LIMIT) {
                Ok(commits) => choices.extend(commits.iter().map(commit_picker_row)),
                Err(error) => self.notice(format!("cannot list HEAD commits: {error}")),
            }
        }
        choices
    }

    /// Commits eligible for a first-parent comparison.
    pub(crate) fn comparison_commit_choices(&mut self) -> Vec<String> {
        if !self.workspace.is_git() {
            return Vec::new();
        }
        let mut choices = Vec::new();
        if self.workspace.head_commit().is_some() {
            choices.push("HEAD".to_owned());
        }
        choices.push("Tags...".to_owned());
        choices.push("Branches...".to_owned());
        if self.workspace.head_commit().is_some() {
            match self.workspace.recent_commits(0, COMMIT_PICKER_LIMIT) {
                Ok(commits) => choices.extend(commits.iter().map(commit_picker_row)),
                Err(error) => self.notice(format!("cannot list HEAD commits: {error}")),
            }
        }
        choices
    }

    /// Tags that can be pinned as one comparison side.
    pub(crate) fn comparison_tag_choices(&mut self) -> Vec<String> {
        match self.workspace.revision_choices() {
            Ok(choices) => choices
                .into_iter()
                .filter(|choice| choice.kind() == RevisionChoiceKind::Tag)
                .map(|choice| format!("tag {}", choice.name()))
                .collect(),
            Err(error) => {
                self.notice(format!("cannot list tags: {error}"));
                Vec::new()
            }
        }
    }

    /// Local and remote-tracking branches that can supply commit history.
    pub(crate) fn comparison_branch_choices(&mut self) -> Vec<String> {
        match self.workspace.revision_choices() {
            Ok(choices) => choices
                .into_iter()
                .filter(|choice| choice.kind() == RevisionChoiceKind::Branch)
                .map(|choice| {
                    if choice.is_remote_branch() {
                        format!("remote branch {}", choice.name())
                    } else {
                        format!("branch {}", choice.name())
                    }
                })
                .collect(),
            Err(error) => {
                self.notice(format!("cannot list branches: {error}"));
                Vec::new()
            }
        }
    }

    /// The newest commits reachable from `branch`.
    pub(crate) fn comparison_branch_commit_choices(&mut self, branch: &str) -> Vec<String> {
        match self.workspace.commits_from(branch, 0, COMMIT_PICKER_LIMIT) {
            Ok(commits) => commits.iter().map(commit_picker_row).collect(),
            Err(error) => {
                self.notice(format!("cannot list commits on {branch}: {error}"));
                Vec::new()
            }
        }
    }

    /// Saved review points offered through their own base-only menu.
    pub(crate) fn comparison_review_point_choices(&self) -> Vec<String> {
        self.review_points
            .as_ref()
            .into_iter()
            .flat_map(ReviewPointStore::list)
            .map(|point| {
                format!(
                    "review point {}{}",
                    point.id(),
                    point
                        .name()
                        .map_or_else(String::new, |name| format!(" ({name})"))
                )
            })
            .collect()
    }

    /// Resolve an ID-prefix search beyond the menu's bounded commit page.
    pub(crate) fn complete_picker_commit_search(&mut self, search: &super::CommitSearch) {
        let commits = match search.revision.as_deref() {
            Some(revision) => self
                .workspace
                .commits_from_matching_prefix(revision, &search.prefix),
            None => self.workspace.commits_matching_prefix(&search.prefix),
        };
        match commits {
            Ok(commits) => {
                let rows = commits.iter().map(commit_picker_row).collect();
                if let Some(picker) = self.picker_mut() {
                    picker.set_commit_search(&search.prefix, rows);
                }
            }
            Err(error) => self.notice(format!("cannot search commit IDs: {error}")),
        }
    }

    /// Continue or complete one nested comparison picker.
    pub(crate) fn choose_nested_comparison(
        &mut self,
        kind: super::PickerKind,
        item: &str,
        input: &str,
    ) {
        match kind {
            super::PickerKind::ComparisonBranches(side) => {
                let Some(branch) = branch_name_from_row(item) else {
                    self.notice("choose a branch");
                    return;
                };
                let choices = self.comparison_branch_commit_choices(branch);
                self.open_scoped_picker(
                    super::PickerKind::ComparisonBranchCommits(side),
                    choices,
                    Some(branch.to_owned()),
                );
            }
            super::PickerKind::ComparisonTags(side)
            | super::PickerKind::ComparisonBranchCommits(side)
            | super::PickerKind::ComparisonAdvanced(side) => {
                self.choose_diff_side_input(side.picker_kind(), item, input);
            }
            super::PickerKind::ComparisonReviewPoints => {
                if !self.reload_review_points() {
                    return;
                }
                self.choose_diff_side_input(super::PickerKind::ComparisonBase, item, input);
            }
            _ => {}
        }
    }

    /// Resolve a picker entry or typed local Git revision.
    pub(crate) fn resolve_comparison_endpoint(
        &self,
        value: &str,
    ) -> Result<ComparisonEndpoint, String> {
        let value = value.trim();
        match value.to_ascii_lowercase().as_str() {
            "working tree" | "working-tree" | "worktree" => {
                return Ok(ComparisonEndpoint::WorkingTree);
            }
            "index" => return Ok(ComparisonEndpoint::Index),
            "empty tree" | "empty-tree" => return Ok(ComparisonEndpoint::EmptyTree),
            _ => {}
        }
        if let Some(id) = value.strip_prefix("review point ") {
            let id = id.split_whitespace().next().unwrap_or(id);
            if self
                .review_points
                .as_ref()
                .and_then(|store| store.get(id))
                .is_some()
            {
                return Ok(ComparisonEndpoint::ReviewPoint(id.to_owned()));
            }
            return Err(format!("review point {id} is unavailable"));
        }
        if let Some(name) = value.strip_prefix("tag ") {
            let revision = format!("refs/tags/{name}");
            return self
                .workspace
                .resolve_revision(&revision)
                .map(|commit| ComparisonEndpoint::Commit(commit.id()))
                .map_err(|error| format!("cannot resolve tag `{name}`: {error}"));
        }
        let typed_revision = if let Some(id) = commit_id_from_row(value) {
            id
        } else if let Some(value) = value.strip_prefix("commit ") {
            value.split(" · ").next().unwrap_or(value).trim()
        } else if (value.starts_with("branch ") || value.starts_with("tag "))
            && let Some((_, suffix)) = value.rsplit_once(" (")
        {
            suffix.strip_suffix(')').unwrap_or(suffix)
        } else {
            value
        };
        self.workspace
            .resolve_revision(typed_revision)
            .map(|commit| ComparisonEndpoint::Commit(commit.id()))
            .map_err(|error| format!("cannot resolve revision `{value}`: {error}"))
    }

    /// Apply a new immutable or mutable base.
    #[cfg(test)]
    pub(crate) fn set_comparison_base(&mut self, endpoint: ComparisonEndpoint) {
        self.set_comparison_base_aliased(endpoint, None);
    }

    pub(crate) fn set_comparison_base_aliased(
        &mut self,
        endpoint: ComparisonEndpoint,
        alias: Option<EndpointAlias>,
    ) {
        if self.annotation_draft_blocks("changing Base") {
            return;
        }
        let restore = (self.diff_mode == DiffMode::Off).then_some(self.last_active_diff_mode);
        self.comparison.restore_mode = restore;
        self.comparison.set_base_aliased(
            endpoint,
            alias,
            &mut self.workspace,
            self.review_points.as_ref(),
        );
        if let Some(error) = self.comparison.error().map(str::to_owned) {
            self.notice(error);
        } else if let Some(mode) = restore {
            self.activate_diff_mode(mode);
        } else {
            self.apply_refreshed_comparison(false);
        }
    }

    /// Apply a new immutable or mutable target.
    #[cfg(test)]
    pub(crate) fn set_comparison_target(&mut self, endpoint: ComparisonEndpoint) {
        self.set_comparison_target_aliased(endpoint, None);
    }

    pub(crate) fn set_comparison_target_aliased(
        &mut self,
        endpoint: ComparisonEndpoint,
        alias: Option<EndpointAlias>,
    ) {
        if self.annotation_draft_blocks("changing Target") {
            return;
        }
        if self.diff_mode == DiffMode::Off {
            self.comparison.select_target_aliased(endpoint, alias);
            self.refresh_comparison();
            return;
        }
        self.comparison.set_target_aliased(
            endpoint,
            alias,
            &mut self.workspace,
            self.review_points.as_ref(),
        );
        if let Some(error) = self.comparison.error().map(str::to_owned) {
            self.notice(error);
        } else {
            self.apply_refreshed_comparison(false);
        }
    }

    fn select_comparison_endpoints(
        &mut self,
        base: ComparisonEndpoint,
        base_alias: Option<EndpointAlias>,
        target: ComparisonEndpoint,
        target_alias: Option<EndpointAlias>,
    ) -> (bool, bool) {
        let restore = (self.diff_mode == DiffMode::Off).then_some(self.last_active_diff_mode);
        self.comparison.restore_mode = restore;
        let persisted = self.comparison.set_endpoints_aliased(
            base,
            base_alias,
            target,
            target_alias,
            &mut self.workspace,
            self.review_points.as_ref(),
        );
        let available = self.comparison.error().is_none() || self.comparison.pending();
        if let Some(error) = self.comparison.error().map(str::to_owned) {
            self.notice(error);
        } else if let Some(mode) = restore {
            self.activate_diff_mode(mode);
        } else {
            self.apply_refreshed_comparison(false);
        }
        (available, persisted)
    }

    /// Pin Base to the current `HEAD` and select the working tree as Target.
    pub(crate) fn select_head_working_tree(&mut self) {
        if self.annotation_draft_blocks("changing comparison") {
            return;
        }
        let (base, alias) = head_endpoint(&self.workspace);
        let _ =
            self.select_comparison_endpoints(base, alias, ComparisonEndpoint::WorkingTree, None);
    }

    /// Compare one immutable commit with its first parent.
    pub(crate) fn select_commit_parent(
        &mut self,
        target: &CommitId,
        target_alias: Option<EndpointAlias>,
    ) {
        if self.annotation_draft_blocks("changing comparison") {
            return;
        }
        let commit = match self.workspace.resolve_revision(target.as_str()) {
            Ok(commit) => commit,
            Err(error) => {
                self.notice(format!("cannot read selected commit: {error}"));
                return;
            }
        };
        let Some(parent) = commit.parents().next() else {
            self.notice(format!("commit {} has no parent", commit.short()));
            return;
        };
        let base_alias =
            matches!(target_alias, Some(EndpointAlias::Head)).then_some(EndpointAlias::HeadParent);
        let _ = self.select_comparison_endpoints(
            ComparisonEndpoint::Commit(parent),
            base_alias,
            ComparisonEndpoint::Commit(commit.id()),
            target_alias,
        );
    }

    /// Compare the first parent of the current `HEAD` with `HEAD`.
    pub(crate) fn select_head_parent(&mut self) {
        if self.annotation_draft_blocks("changing comparison") {
            return;
        }
        let target = match self.workspace.resolve_revision("HEAD") {
            Ok(commit) => commit.id(),
            Err(error) => {
                self.notice(format!("HEAD~1 to HEAD is unavailable: {error}"));
                return;
            }
        };
        self.select_commit_parent(&target, Some(EndpointAlias::Head));
    }

    /// Use a newly captured immutable review point as Base against Working tree.
    pub(crate) fn select_review_point(&mut self, id: String) -> (bool, bool) {
        self.select_comparison_endpoints(
            ComparisonEndpoint::ReviewPoint(id),
            None,
            ComparisonEndpoint::WorkingTree,
            None,
        )
    }

    /// Cached selected-comparison facts for navigation and rendering.
    pub(crate) fn comparison_status(&self) -> &Status {
        &self.comparison_status
    }

    /// Build selected-comparison facts once for the current generation.
    pub(crate) fn rebuild_comparison_status(&mut self) {
        self.change_stop = None;
        self.comparison_status = self.compute_comparison_status();
    }

    fn compute_comparison_status(&self) -> Status {
        if self.plain_default_comparison() {
            return Status::default();
        }
        let Some(comparison) = self.comparison.current() else {
            return Status::default();
        };
        let entries = comparison
            .changes()
            .iter()
            .map(|change| {
                let state = match change.kind() {
                    PathChangeKind::Added
                        if self
                            .status
                            .get(change.path())
                            .is_some_and(|entry| entry.changes().is_only_untracked()) =>
                    {
                        GitState::Untracked
                    }
                    PathChangeKind::Added => GitState::Added,
                    PathChangeKind::Deleted => GitState::Deleted,
                    PathChangeKind::Missing
                    | PathChangeKind::Binary
                    | PathChangeKind::Unsupported
                    | PathChangeKind::TypeChanged
                    | PathChangeKind::ModeChanged
                    | PathChangeKind::ContentChanged => GitState::Modified,
                };
                let (added, removed) = self.comparison_line_counts(change.path()).unwrap_or((0, 0));
                let entry = Entry::new(
                    change.path().to_path_buf(),
                    Changes::Unstaged(state),
                    added,
                    removed,
                );
                if matches!(change.kind(), PathChangeKind::Binary) {
                    entry.binary()
                } else {
                    entry
                }
            })
            .collect();
        Status::from_entries(entries)
    }

    /// Selected comparison paths absent from the active checkout.
    pub(crate) fn comparison_virtual_paths(&self) -> Vec<PathBuf> {
        if self.diff_mode == DiffMode::Off {
            return self
                .off_target_paths
                .iter()
                .flatten()
                .filter(|path| !self.workspace.root().join(path).is_file())
                .cloned()
                .collect();
        }
        let Some(comparison) = self.comparison.current() else {
            return Vec::new();
        };
        comparison
            .changes()
            .iter()
            .map(fathomable_core::diff::PathChange::path)
            // A selected deletion must survive until its background comparison lands.
            .chain(
                self.tree
                    .as_ref()
                    .and_then(|tree| tree.current())
                    .map(fathomable_core::tree::Row::path)
                    .filter(|path| {
                        comparison
                            .target_paths()
                            .iter()
                            .any(|target| target == path)
                    }),
            )
            .filter(|path| !self.workspace.root().join(path).is_file())
            .map(Path::to_path_buf)
            .collect()
    }

    /// The selected non-working target's complete file boundary.
    pub(crate) fn comparison_snapshot_paths(&self) -> Option<Vec<PathBuf>> {
        if self.diff_mode == DiffMode::Off {
            return (self.comparison.target() != &ComparisonEndpoint::WorkingTree)
                .then(|| self.off_target_paths.clone().unwrap_or_default());
        }
        let comparison = self.comparison.current()?;
        if comparison.target() == &ComparisonEndpoint::WorkingTree {
            return None;
        }
        let mut paths = comparison.target_paths().to_vec();
        paths.extend(
            comparison
                .changes()
                .iter()
                .filter(|change| matches!(change.target(), PathState::Absent))
                .map(|change| change.path().to_path_buf()),
        );
        paths.sort();
        paths.dedup();
        Some(paths)
    }

    /// The selected comparison classification for a path.
    pub(crate) fn comparison_kind(&self, path: &Path) -> Option<PathChangeKind> {
        if self.diff_mode == DiffMode::Off {
            return None;
        }
        self.comparison.current().and_then(|comparison| {
            comparison
                .changes()
                .iter()
                .find(|change| change.path() == path)
                .map(fathomable_core::diff::PathChange::kind)
        })
    }

    fn comparison_line_counts(&self, path: &Path) -> Result<(usize, usize), String> {
        self.comparison
            .counts
            .get(path)
            .copied()
            .ok_or_else(|| "comparison line counts unavailable".to_owned())
    }

    /// Update a loaded view to the selected target and base.
    #[expect(
        clippy::too_many_lines,
        reason = "projection keeps each endpoint and content-state case in one ordered flow"
    )]
    pub(crate) fn apply_comparison_projection(&mut self, index: usize) {
        if self.diff_mode == DiffMode::Off {
            if let Err(error) = self.apply_off_projection(index) {
                self.notice(error);
            }
            return;
        }
        if self.comparison.stale() {
            return;
        }
        if self.plain_default_comparison() {
            self.docs[index].view.clear_comparison_body();
            return;
        }
        let Some(comparison) = self.comparison.current().cloned() else {
            return;
        };
        let path = self.docs[index].relative.clone();
        if self.comparison.target() == &ComparisonEndpoint::WorkingTree
            && self.docs[index].deleted == Some(super::Deleted::Loaded)
            && !comparison
                .changes()
                .iter()
                .any(|change| change.path() == path)
        {
            self.docs[index].comparison_notice = None;
            self.docs[index].view.set_comparison_body(DiffBody::Diff {
                base: Text::Owned(String::new()),
                target: Text::Owned(String::new()),
            });
            return;
        }

        if let Some(change) = comparison
            .changes()
            .iter()
            .find(|change| change.path() == path)
            && matches!(
                change.kind(),
                PathChangeKind::Binary | PathChangeKind::Unsupported | PathChangeKind::Missing
            )
        {
            let notice = match change.kind() {
                PathChangeKind::Binary => "binary content is not shown as text".to_owned(),
                PathChangeKind::Unsupported => "comparison path type is unsupported".to_owned(),
                PathChangeKind::Missing => "comparison endpoint content is unavailable".to_owned(),
                _ => unreachable!("comparison kind checked above"),
            };
            self.docs[index].comparison_notice = Some(notice.clone());
            self.docs[index]
                .view
                .set_comparison_body(DiffBody::Notice(notice));
            return;
        }
        let metadata_notice = comparison
            .changes()
            .iter()
            .find(|change| change.path() == path)
            .filter(|change| !change.content_changed())
            .map(|change| match change.kind() {
                PathChangeKind::ModeChanged => "file mode changed; no text hunk".to_owned(),
                PathChangeKind::TypeChanged => "file type changed; no text hunk".to_owned(),
                _ => "comparison metadata changed; no text hunk".to_owned(),
            });
        let base = self.comparison_endpoint_text(comparison.base(), &path);
        let target = self.comparison_endpoint_text(comparison.target(), &path);
        let (mut base, target) = match (base, target) {
            (Ok(base), Ok(target)) => (base, target),
            (Err(error), _) | (_, Err(error)) => {
                self.docs[index].comparison_notice = Some(error.clone());
                self.docs[index]
                    .view
                    .set_comparison_body(DiffBody::Notice(error));
                return;
            }
        };
        if base.is_none()
            && comparison
                .changes()
                .iter()
                .find(|change| change.path() == path)
                .is_some_and(|change| matches!(change.base(), PathState::Absent))
        {
            base = Some(String::new());
        }
        let target_missing = target.is_none();
        let body = DiffBody::Diff {
            base: Text::Owned(base.clone().unwrap_or_default()),
            target: Text::Owned(target.clone().unwrap_or_default()),
        };
        let display = target.clone().or_else(|| base.clone()).unwrap_or_default();
        self.docs[index].comparison_notice = metadata_notice;
        let index_text = (comparison.target() == &ComparisonEndpoint::WorkingTree)
            .then(|| {
                self.workspace
                    .endpoint_text(&ComparisonEndpoint::Index, &path)
                    .ok()
                    .flatten()
            })
            .flatten();
        self.docs[index].view.set_compare(self.comparison.compare());
        self.docs[index].view.set_bases(index_text, base.clone());
        self.docs[index].view.set_comparison_body(body);
        let changed = self.docs[index].view.reload(display);
        if target_missing {
            self.docs[index].deleted = Some(super::Deleted::ComparisonBase);
        } else if self.docs[index].deleted == Some(super::Deleted::ComparisonBase) {
            self.docs[index].deleted = None;
        }
        self.docs[index].view.set_worktree_missing(target_missing);
        if changed {
            self.queue_highlight(index);
        }
    }

    fn plain_default_comparison(&self) -> bool {
        !self.workspace.is_git()
            && self.comparison.base() == &ComparisonEndpoint::EmptyTree
            && self.comparison.target() == &ComparisonEndpoint::WorkingTree
    }

    /// Remove retained point-backed rows after a non-Git fallback.
    pub(crate) fn repair_review_point_fallback_projection(&mut self) {
        if !self.plain_default_comparison() {
            return;
        }
        for index in 0..self.docs.len() {
            let path = self.docs[index].relative.clone();
            let target = self
                .workspace
                .endpoint_text(&ComparisonEndpoint::WorkingTree, &path)
                .ok()
                .flatten();
            self.docs[index].view.clear_comparison_body();
            self.docs[index].view.set_bases(None, None);
            self.docs[index].comparison_notice = target
                .is_none()
                .then(|| "not present in WorkingTree".to_owned());
            self.docs[index].deleted = None;
            self.docs[index].view.set_worktree_missing(target.is_none());
            let changed = self.docs[index].view.reload(target.unwrap_or_default());
            if changed {
                self.queue_highlight(index);
            }
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod regression_tests;
