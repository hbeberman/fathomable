// @okf-doc: /decisions/0087-global-comparisons-and-board-history.md
//! Viewer-wide comparison selection and explicit review-point focus.
//!
//! The app owns one comparison for the active checkout.  File views only
//! render the projection of that selection; they do not select their own
//! pair of endpoints.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fathomable_core::diff::{Compare, Comparison, PathChangeKind, PathState};
use fathomable_core::review_points::{ReviewPoint, ReviewPointStore};
use fathomable_core::status::{Changes, Entry, State as GitState, Status};
use fathomable_core::workspace::{CommitId, ComparisonEndpoint, Workspace};
use serde::{Deserialize, Serialize};

use super::App;
use super::diff::{DiffBody, Text};

const PREFERENCE_FILE: &str = "comparison.json";

/// Which delta the viewer lists and renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Focus {
    /// The selected comparison pair.
    AllChanges,
    /// The saved review-point-to-working-tree delta.
    Since(String),
}

/// One app-owned comparison selection and its last successful result.
#[derive(Debug)]
pub(crate) struct State {
    base: ComparisonEndpoint,
    target: ComparisonEndpoint,
    focus: Focus,
    compare: Compare,
    preference: PathBuf,
    persisted: bool,
    generation: u64,
    observed_head: Option<String>,
    current: Option<Comparison>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Preference {
    base: String,
    target: String,
    focus: Option<String>,
    whitespace: bool,
}

impl State {
    /// Create the default selection, preferring a persisted checkout choice.
    pub(crate) fn load(
        preference_dir: impl AsRef<Path>,
        workspace: &Workspace,
        compare: Compare,
    ) -> Self {
        let preference_dir = preference_dir.as_ref().to_path_buf();
        let preference = preference_dir.join(PREFERENCE_FILE);
        let default_base = workspace
            .head_commit()
            .and_then(|head| CommitId::parse(head).ok())
            .map_or(ComparisonEndpoint::EmptyTree, ComparisonEndpoint::Commit);
        let mut state = Self {
            base: default_base,
            target: ComparisonEndpoint::WorkingTree,
            focus: Focus::AllChanges,
            compare,
            preference,
            persisted: false,
            generation: 0,
            observed_head: workspace.head_commit(),
            current: None,
            error: None,
        };
        if let Ok(bytes) = fs::read(&state.preference)
            && let Ok(saved) = serde_json::from_slice::<Preference>(&bytes)
        {
            state.persisted = true;
            if let Some(base) = parse_endpoint(&saved.base) {
                state.base = base;
            }
            if let Some(target) = parse_endpoint(&saved.target) {
                state.target = target;
            }
            state.focus = saved.focus.map_or(Focus::AllChanges, Focus::Since);
            state.compare.whitespace = if saved.whitespace {
                fathomable_core::diff::Whitespace::Ignore
            } else {
                fathomable_core::diff::Whitespace::Exact
            };
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

    /// The selected temporal focus.
    pub(crate) fn focus(&self) -> &Focus {
        &self.focus
    }

    /// The current line-diff settings.
    pub(crate) const fn compare(&self) -> Compare {
        self.compare
    }

    /// The last successfully computed effective comparison.
    pub(crate) fn current(&self) -> Option<&Comparison> {
        self.current.as_ref()
    }

    /// A visible error from the newest refresh, if any.
    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether the displayed comparison is from an older successful refresh.
    pub(crate) fn stale(&self) -> bool {
        self.error.is_some() && self.current.is_some()
    }

    /// A monotonically increasing refresh generation.
    #[expect(
        dead_code,
        reason = "the generation guards future asynchronous refresh delivery"
    )]
    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    /// Refresh mutable endpoints without discarding the last good result.
    pub(crate) fn refresh(
        &mut self,
        workspace: &mut Workspace,
        review_points: Option<&ReviewPointStore>,
    ) {
        self.generation = self.generation.wrapping_add(1);
        let result = match &self.focus {
            Focus::AllChanges => match (&self.base, &self.target) {
                (ComparisonEndpoint::ReviewPoint(id), ComparisonEndpoint::WorkingTree) => {
                    review_points
                        .ok_or_else(|| format!("review point {id} is unavailable"))
                        .and_then(|store| {
                            store
                                .get(id)
                                .ok_or_else(|| format!("review point {id} is unavailable"))
                                .and_then(|point| {
                                    store
                                        .compare_to_working(point, workspace)
                                        .map_err(|error| error.to_string())
                                })
                        })
                }
                (ComparisonEndpoint::ReviewPoint(_), _)
                | (_, ComparisonEndpoint::ReviewPoint(_)) => {
                    Err("review points compare only against the working tree".to_owned())
                }
                _ => workspace
                    .compare(self.base.clone(), self.target.clone())
                    .map_err(|error| error.to_string()),
            },
            Focus::Since(id) => match review_points {
                None => Err(format!("review point {id} is unavailable")),
                Some(store) => match store.get(id) {
                    None => Err(format!("review point {id} is unavailable")),
                    Some(_point) if self.target != ComparisonEndpoint::WorkingTree => {
                        Err("Since review point focus requires a working-tree target".to_owned())
                    }
                    Some(point) => store
                        .compare_to_working(point, workspace)
                        .map_err(|error| error.to_string()),
                },
            },
        };
        let result = result.and_then(|comparison| {
            let unavailable = comparison
                .changes()
                .iter()
                .find(|change| change.kind() == PathChangeKind::Missing)
                .map(|change| {
                    let detail = match (change.base(), change.target()) {
                        (PathState::Missing(detail), _) | (_, PathState::Missing(detail)) => detail,
                        _ => "comparison endpoint content is unavailable",
                    };
                    format!("{}: {detail}", change.path().display())
                });
            unavailable.map_or(Ok(comparison), Err)
        });
        match result {
            Ok(comparison) => {
                self.current = Some(comparison);
                self.error = None;
                if !self.persisted {
                    self.persist();
                }
            }

            Err(error) => {
                self.error = Some(error);
            }
        }
        self.observed_head = workspace.head_commit();
    }

    /// Change the base and clear an incompatible temporal focus.
    pub(crate) fn set_base(
        &mut self,
        base: ComparisonEndpoint,
        workspace: &mut Workspace,
        review_points: Option<&ReviewPointStore>,
    ) {
        let previous = (self.base.clone(), self.target.clone(), self.focus.clone());
        let had_good = self.current.is_some();
        self.base = base;
        self.refresh(workspace, review_points);
        if self.error.is_some() {
            if had_good {
                (self.base, self.target, self.focus) = previous;
            }
            return;
        }
        self.persist();
    }

    /// Change the target and clear an incompatible temporal focus.
    pub(crate) fn set_target(
        &mut self,
        target: ComparisonEndpoint,
        workspace: &mut Workspace,
        review_points: Option<&ReviewPointStore>,
    ) {
        let previous = (self.base.clone(), self.target.clone(), self.focus.clone());
        let had_good = self.current.is_some();
        self.target = target;
        if self.target != ComparisonEndpoint::WorkingTree {
            self.focus = Focus::AllChanges;
        }
        self.refresh(workspace, review_points);
        if self.error.is_some() {
            if had_good {
                (self.base, self.target, self.focus) = previous;
            }
            return;
        }
        self.persist();
    }

    /// Select All changes or a saved review point.
    pub(crate) fn set_focus(
        &mut self,
        focus: Focus,
        workspace: &mut Workspace,
        review_points: Option<&ReviewPointStore>,
    ) {
        let previous = (self.base.clone(), self.target.clone(), self.focus.clone());
        let had_good = self.current.is_some();
        if matches!(focus, Focus::Since(_)) && self.target != ComparisonEndpoint::WorkingTree {
            self.focus = Focus::AllChanges;
            self.error = Some("Since review point focus requires a working-tree target".to_owned());
        } else {
            self.focus = focus;
            self.refresh(workspace, review_points);
        }
        if self.error.is_some() {
            if had_good {
                (self.base, self.target, self.focus) = previous;
            }
            return;
        }
        self.persist();
    }

    /// Toggle exact versus whitespace-insensitive line comparison.
    pub(crate) fn toggle_whitespace(&mut self, workspace: &mut Workspace) {
        self.compare.whitespace = match self.compare.whitespace {
            fathomable_core::diff::Whitespace::Exact => fathomable_core::diff::Whitespace::Ignore,
            fathomable_core::diff::Whitespace::Ignore => fathomable_core::diff::Whitespace::Exact,
        };
        self.persist();
        let _ = workspace;
    }

    /// Persist this viewer's preference outside the repository.
    pub(crate) fn persist(&mut self) {
        let preference = Preference {
            base: endpoint_string(&self.base),
            target: endpoint_string(&self.target),
            focus: match &self.focus {
                Focus::AllChanges => None,
                Focus::Since(id) => Some(id.clone()),
            },
            whitespace: matches!(
                self.compare.whitespace,
                fathomable_core::diff::Whitespace::Ignore
            ),
        };
        let Ok(bytes) = serde_json::to_vec_pretty(&preference) else {
            return;
        };
        if let Some(parent) = self.preference.parent()
            && let Err(error) = fs::create_dir_all(parent)
        {
            tracing::warn!(%error, path = %parent.display(), "cannot create comparison preference directory");
            return;
        }
        let temporary = self
            .preference
            .with_extension(format!("{}.tmp", std::process::id()));
        match write_atomic(&temporary, &self.preference, &bytes) {
            Ok(()) => self.persisted = true,
            Err(error) => {
                tracing::warn!(%error, path = %self.preference.display(), "cannot persist comparison preference");
            }
        }
    }

    /// Whether the observed branch/HEAD changed since the last refresh.
    pub(crate) fn head_changed(&self, workspace: &Workspace) -> bool {
        self.target == ComparisonEndpoint::WorkingTree
            && self.observed_head.as_deref() != workspace.head_commit().as_deref()
    }
}

fn write_atomic(temporary: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(temporary, bytes)?;
    fs::rename(temporary, target)?;
    Ok(())
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

impl App {
    /// The effective comparison result, if the last refresh succeeded.
    pub(crate) fn comparison(&self) -> Option<&Comparison> {
        self.comparison.current()
    }

    /// The currently selected temporal focus.
    #[cfg(test)]
    pub(crate) fn comparison_state_focus(&self) -> Focus {
        self.comparison.focus().clone()
    }

    /// The comparison selection error retained alongside its last good view.
    #[cfg(test)]
    pub(crate) fn comparison_state_error(&self) -> Option<&str> {
        self.comparison.error()
    }

    /// The current comparison preference and focus label.
    pub(crate) fn comparison_label(&self) -> String {
        let state = &self.comparison;
        let mut label = format!("Compare: {} → {}", state.base(), state.target());
        if let Focus::Since(id) = state.focus() {
            let name = self
                .review_points
                .as_ref()
                .and_then(|store| store.get(id))
                .and_then(ReviewPoint::name)
                .map_or_else(|| id.clone(), str::to_owned);
            let _ = write!(label, " · Since {name:?}");
        } else {
            label.push_str(" · All changes");
        }
        if state.stale() {
            label.push_str(" · stale");
        }
        label
    }

    /// Compact comparison provenance for the status line.
    pub(crate) fn comparison_badge_visible(&self) -> bool {
        self.workspace.is_git() || !self.plain_default_comparison()
    }

    /// Compact comparison provenance for the status line.
    pub(crate) fn comparison_badge(&self) -> String {
        let focus = match self.comparison.focus() {
            Focus::AllChanges => "all".to_owned(),
            Focus::Since(id) => format!("since {}", &id[..id.len().min(8)]),
        };
        format!(
            "CMP {} → {} · {focus}",
            self.comparison.base(),
            self.comparison.target()
        )
    }

    /// Open the unified comparison control.
    pub(crate) fn open_comparison_control(&mut self) {
        self.open_picker(super::PickerKind::ComparisonControl);
    }

    /// Pick the temporal focus of the current comparison.
    pub(crate) fn pick_comparison_focus(&mut self) {
        self.open_picker(super::PickerKind::ComparisonFocus);
    }

    /// Save the selected comparison preference after a state change.
    pub(crate) fn persist_comparison(&mut self) {
        self.comparison.persist();
    }

    /// A repository-wide endpoint picker list.
    pub(crate) fn comparison_choices(&mut self, target: bool) -> Vec<String> {
        let mut choices = vec![
            "working tree".to_owned(),
            "index".to_owned(),
            "empty tree".to_owned(),
        ];
        if self.workspace.is_git() {
            if let Ok(revisions) = self.workspace.revision_choices() {
                choices.extend(revisions.into_iter().map(|choice| {
                    format!("{} {} ({})", choice.kind(), choice.name(), choice.commit())
                }));
            }
            if let Ok(commits) = self.workspace.recent_commits(0, 50) {
                choices.extend(commits.into_iter().map(|commit| {
                    format!(
                        "commit {} · {} · {}",
                        commit.short(),
                        commit.subject(),
                        super::draw::format_age(commit.time(), fathomable_core::clock::now())
                    )
                }));
            }
        }
        if !target && let Some(store) = &self.review_points {
            choices.extend(store.list().into_iter().map(|point| {
                format!(
                    "review point {}{}",
                    point.id(),
                    point
                        .name()
                        .map_or_else(String::new, |name| format!(" ({name})"))
                )
            }));
        }
        choices
    }

    /// The single control's actions.
    pub(crate) fn comparison_control_choices(&self) -> Vec<String> {
        vec![
            "open unified diff".to_owned(),
            format!("base: {}", self.comparison.base()),
            format!("target: {}", self.comparison.target()),
            match self.comparison.focus() {
                Focus::AllChanges => "focus: All changes".to_owned(),
                Focus::Since(id) => format!("focus: Since {id}"),
            },
            "save review point".to_owned(),
            "start comparison at current HEAD".to_owned(),
            "select contiguous commit batch (type first..last)".to_owned(),
            if self.comparison.compare().whitespace == fathomable_core::diff::Whitespace::Ignore {
                "whitespace: ignored".to_owned()
            } else {
                "whitespace: exact".to_owned()
            },
        ]
    }

    /// The focus picker list.
    pub(crate) fn comparison_focus_choices(&self) -> Vec<String> {
        let mut choices = vec!["All changes".to_owned()];
        if let Some(store) = &self.review_points {
            choices.extend(store.list().into_iter().map(|point| {
                format!(
                    "Since {} [{}]",
                    point.name().unwrap_or(point.id()),
                    point.id()
                )
            }));
        }
        choices
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
        let typed_revision = if let Some(value) = value.strip_prefix("commit ") {
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
    pub(crate) fn set_comparison_base(&mut self, endpoint: ComparisonEndpoint) {
        self.comparison
            .set_base(endpoint, &mut self.workspace, self.review_points.as_ref());
        if let Some(error) = self.comparison.error().map(str::to_owned) {
            self.notice(error);
        } else {
            self.refresh_comparison();
        }
    }

    /// Apply a new immutable or mutable target.
    pub(crate) fn set_comparison_target(&mut self, endpoint: ComparisonEndpoint) {
        self.comparison
            .set_target(endpoint, &mut self.workspace, self.review_points.as_ref());
        if let Some(error) = self.comparison.error().map(str::to_owned) {
            self.notice(error);
        } else {
            self.refresh_comparison();
        }
    }

    /// Set the current focus from the focus picker.
    pub(crate) fn choose_comparison_focus(&mut self, item: &str) {
        if item == "All changes" {
            self.comparison.set_focus(
                Focus::AllChanges,
                &mut self.workspace,
                self.review_points.as_ref(),
            );
        } else if let Some(id) = item
            .split('[')
            .nth(1)
            .and_then(|value| value.strip_suffix(']'))
        {
            self.comparison.set_focus(
                Focus::Since(id.to_owned()),
                &mut self.workspace,
                self.review_points.as_ref(),
            );
        } else {
            self.notice("choose All changes or a saved review point");
            return;
        }
        if let Some(error) = self.comparison.error().map(str::to_owned) {
            self.notice(error);
        } else {
            self.refresh_comparison();
        }
    }

    /// Apply a control-list action.
    pub(crate) fn choose_comparison_control(&mut self, item: &str) {
        if item == "open unified diff" {
            self.show_comparison_diff();
        } else if item.starts_with("base:") {
            self.pick_diff_side(false);
        } else if item.starts_with("target:") {
            self.pick_diff_side(true);
        } else if item.starts_with("focus:") {
            self.pick_comparison_focus();
        } else if item == "save review point" {
            self.request_review_point();
        } else if item == "start comparison at current HEAD" {
            self.start_comparison_at_head();
        } else if item.starts_with("select contiguous commit batch") {
            self.pick_diff_side(false);
        } else if item.starts_with("whitespace:") {
            self.toggle_whitespace();
        }
    }

    /// Pin the current HEAD as the comparison base and follow the working tree.
    pub(crate) fn start_comparison_at_head(&mut self) {
        let base = self
            .workspace
            .head_commit()
            .and_then(|head| CommitId::parse(head).ok())
            .map_or(ComparisonEndpoint::EmptyTree, ComparisonEndpoint::Commit);
        self.comparison
            .set_base(base, &mut self.workspace, self.review_points.as_ref());
        self.comparison.set_target(
            ComparisonEndpoint::WorkingTree,
            &mut self.workspace,
            self.review_points.as_ref(),
        );
        self.comparison.set_focus(
            Focus::AllChanges,
            &mut self.workspace,
            self.review_points.as_ref(),
        );
        self.refresh_comparison();
        self.notice("comparison started at current HEAD");
    }

    /// Cached selected-comparison facts for navigation and rendering.
    pub(crate) fn comparison_status(&self) -> &Status {
        &self.comparison_status
    }

    /// Build selected-comparison facts once for the current generation.
    pub(crate) fn rebuild_comparison_status(&mut self) {
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
        let Some(comparison) = self.comparison.current() else {
            return Vec::new();
        };
        comparison
            .changes()
            .iter()
            .filter(|change| !self.workspace.root().join(change.path()).is_file())
            .map(|change| change.path().to_path_buf())
            .collect()
    }

    /// The selected comparison classification for a path.
    pub(crate) fn comparison_kind(&self, path: &Path) -> Option<PathChangeKind> {
        self.comparison.current().and_then(|comparison| {
            comparison
                .changes()
                .iter()
                .find(|change| change.path() == path)
                .map(fathomable_core::diff::PathChange::kind)
        })
    }

    fn comparison_line_counts(&self, path: &Path) -> Result<(usize, usize), String> {
        let comparison = self
            .comparison
            .current()
            .ok_or_else(|| "comparison unavailable".to_owned())?;
        let base = self.comparison_endpoint_text(comparison.base(), path)?;
        let target = self.comparison_endpoint_text(comparison.target(), path)?;
        match (base, target) {
            (Some(base), Some(target)) => Ok(fathomable_core::diff::Diff::compare(
                &base,
                &target,
                self.comparison.compare().whitespace,
            )
            .counts()),
            (None, Some(target)) => Ok((target.lines().count(), 0)),
            (Some(base), None) => Ok((0, base.lines().count())),
            (None, None) => Ok((0, 0)),
        }
    }

    /// Update a loaded view to the selected target and base.
    pub(crate) fn apply_comparison_projection(&mut self, index: usize) {
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
        self.docs[index].view.reload(display);
        self.docs[index].view.set_worktree_missing(target_missing);
    }

    fn plain_default_comparison(&self) -> bool {
        !self.workspace.is_git()
            && self.comparison.base() == &ComparisonEndpoint::EmptyTree
            && self.comparison.target() == &ComparisonEndpoint::WorkingTree
            && matches!(self.comparison.focus(), Focus::AllChanges)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use fathomable_core::XdgDirs;
    use fathomable_core::workspace::{CommitId, ComparisonEndpoint, Workspace};
    use fathomable_testing::{TempDir, git};

    use super::Focus;
    use crate::app::testing::{AppBuilder, press, press_key};
    use crate::app::{PickerKind, Popup};
    use crossterm::event::KeyCode;

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
        app.dirty_next();
        assert_eq!(app.current_path(), Path::new("gone.md"));
        app.dirty_next();
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
        app.show_comparison_diff();
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
    fn review_point_focus_shows_reversions_without_changing_on_save() -> anyhow::Result<()> {
        let dir = repository("comparison-review-point")?;
        let root = dir.0.join("ws");
        fs::write(root.join("a.md"), "one\n")?;
        git::commit_and_stage(&root, &[("a.md", "one\n")])?;
        fs::write(root.join("a.md"), "two\n")?;
        let mut app = AppBuilder::at(&root)
            .unopened()
            .review_points(dir.0.join("points"))
            .build()?;

        app.save_review_point(None);
        let point = app
            .review_points
            .as_ref()
            .and_then(|store| store.list().last().cloned())
            .ok_or_else(|| anyhow::anyhow!("review point was not saved"))?;
        assert!(matches!(
            app.comparison()
                .map(fathomable_core::diff::Comparison::base),
            Some(ComparisonEndpoint::Commit(_))
        ));

        fs::write(root.join("a.md"), "one\n")?;
        app.choose_comparison_focus(&format!("Since {} [{}]", point.id(), point.id()));
        app.refresh_comparison();
        assert!(matches!(
            app.comparison()
                .map(fathomable_core::diff::Comparison::base),
            Some(ComparisonEndpoint::ReviewPoint(_))
        ));
        assert_eq!(app.comparison().map(|c| c.changes().len()), Some(1));
        assert!(matches!(app.comparison_state_focus(), Focus::Since(_)));

        app.set_comparison_target(ComparisonEndpoint::Commit(CommitId::parse(
            "0000000000000000000000000000000000000000",
        )?));
        assert_eq!(app.comparison().map(|c| c.changes().len()), Some(1));
        assert_eq!(
            app.comparison()
                .map(fathomable_core::diff::Comparison::target),
            Some(&ComparisonEndpoint::WorkingTree)
        );
        assert!(matches!(app.comparison_state_focus(), Focus::Since(_)));
        assert!(app.comparison_state_error().is_some());

        app.save_review_point(Some("after revert"));
        assert!(matches!(app.comparison_state_focus(), Focus::Since(_)));
        Ok(())
    }

    #[test]
    fn comparison_controls_use_the_new_space_d_bindings() -> anyhow::Result<()> {
        let dir = repository("comparison-bindings")?;
        let root = dir.0.join("ws");
        fs::write(root.join("a.md"), "one\n")?;
        git::commit_and_stage(&root, &[("a.md", "one\n")])?;
        let mut app = AppBuilder::at(&root).unopened().build()?;
        app.open(Path::new("a.md"));

        press(&mut app, " dd");
        assert!(matches!(
            app.popup(),
            Some(Popup::Picker(picker)) if picker.kind() == PickerKind::ComparisonControl
        ));
        press_key(&mut app, KeyCode::Enter);
        assert!(app.view().diff_view());
        app.leave_diff();
        press(&mut app, " dc");
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
    fn review_point_picker_accepts_an_optional_name() -> anyhow::Result<()> {
        let dir = repository("comparison-point-name")?;
        let root = dir.0.join("ws");
        fs::write(root.join("a.md"), "one\n")?;
        git::commit_and_stage(&root, &[("a.md", "one\n")])?;
        let mut app = AppBuilder::at(&root)
            .unopened()
            .review_points(dir.0.join("points"))
            .build()?;

        press(&mut app, " dcBefore fixes");
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
    fn immutable_comparison_synthesizes_modified_paths_missing_from_checkout() -> anyhow::Result<()>
    {
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
            &ComparisonEndpoint::Commit(CommitId::parse(first)?)
        );
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
        app.show_comparison_diff();
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
}

#[cfg(test)]
mod regression_tests;
