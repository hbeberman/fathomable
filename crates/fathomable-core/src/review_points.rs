// @okf-doc: /decisions/0087-global-comparisons-and-board-history.md
//! Explicit workspace review points with content-addressed manifests.
//!
//! A [`ReviewPointStore`] records a deliberate, repository-wide workspace
//! state. Git supplies unchanged committed content; the store keeps changed
//! eligible bytes once per digest and records deletions as tombstones. A
//! point is reported published only after every required blob and its manifest
//! log record are durable. A manifest write failure with an ambiguous outcome
//! is reported as such. Capture never writes the checkout, index, refs, or Git
//! object database.
//!
//! # Examples
//!
//! ```no_run
//! use std::path::Path;
//!
//! use fathomable_core::review_points::ReviewPointStore;
//! use fathomable_core::workspace::Workspace;
//!
//! let mut workspace = Workspace::discover(".")?;
//! let mut store = ReviewPointStore::open(Path::new("review-points"))?;
//! let result = store.capture(&mut workspace, Some("before fixes"))?;
//! if let Some(point) = result.point() {
//!     let bytes = store.load_bytes(point, &workspace, Path::new("README.md"))?;
//!     assert!(bytes.is_some());
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::content;
use crate::diff::{Comparison, FileMode, PathChange, PathInfo, PathState};
use crate::workspace::{
    CommitId, ComparisonEndpoint, EndpointFile, Filter, Workspace, WorkspaceError,
};

const INDEX_FILE: &str = "review-points.jsonl";
const BLOB_DIR: &str = "blobs";
const EVENT_NAME: &str = "review-point";
const DELETE_EVENT_NAME: &str = "review-point-delete";
const RENAME_EVENT_NAME: &str = "review-point-rename";
const MAX_NAME_SCALARS: usize = 128;

/// One deliberate workspace state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPoint {
    id: String,
    name: Option<String>,
    name_revision: u64,
    created: u64,
    checkout: PathBuf,
    workspace_key: PathBuf,
    head: Option<CommitId>,
    entries: Vec<ReviewEntry>,
    issues: Vec<CaptureIssue>,
}

impl ReviewPoint {
    /// The stable identifier of this review point.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The optional human name.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Capture time in Unix seconds.
    #[must_use]
    pub const fn created(&self) -> u64 {
        self.created
    }

    /// The checkout root that was captured.
    #[must_use]
    pub fn checkout(&self) -> &Path {
        &self.checkout
    }

    /// The workspace identity used for shared state.
    #[must_use]
    pub fn workspace_key(&self) -> &Path {
        &self.workspace_key
    }

    /// The observed committed baseline, or `None` for an unborn/non-Git point.
    #[must_use]
    pub fn head(&self) -> Option<&CommitId> {
        self.head.as_ref()
    }

    /// Changed and deleted manifest entries.
    #[must_use]
    pub fn entries(&self) -> &[ReviewEntry] {
        &self.entries
    }

    /// Capture exclusions or other explicitly recorded conditions.
    #[must_use]
    pub fn issues(&self) -> &[CaptureIssue] {
        &self.issues
    }

    /// Whether the eligible manifest is complete enough to select.
    ///
    /// Known policy exclusions are retained as facts and do not invalidate
    /// the point. Read, race, unsupported-content, and missing-object
    /// conditions do invalidate it and are never published.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.issues
            .iter()
            .all(|issue| issue.kind == CaptureIssueKind::Excluded)
    }

    fn entry(&self, path: &Path) -> Option<&ReviewEntry> {
        self.entries
            .binary_search_by(|entry| entry.path.as_path().cmp(path))
            .ok()
            .map(|index| &self.entries[index])
    }
}

/// One changed path in a review-point manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewEntry {
    path: PathBuf,
    mode: FileMode,
    size: Option<u64>,
    content: ReviewContent,
}

impl ReviewEntry {
    /// The root-relative path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The captured mode/type fact.
    #[must_use]
    pub const fn mode(&self) -> FileMode {
        self.mode
    }

    /// The captured byte size, when applicable.
    #[must_use]
    pub const fn size(&self) -> Option<u64> {
        self.size
    }

    /// The content digest, or `None` for a deletion tombstone.
    #[must_use]
    pub fn blob(&self) -> Option<&str> {
        match &self.content {
            ReviewContent::Blob(blob) => Some(blob),
            ReviewContent::Tombstone => None,
        }
    }

    /// Whether this entry explicitly removes a baseline path.
    #[must_use]
    pub const fn is_tombstone(&self) -> bool {
        matches!(self.content, ReviewContent::Tombstone)
    }
}

/// A capture condition that is retained for honest review-point reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureIssue {
    path: Option<PathBuf>,
    kind: CaptureIssueKind,
    detail: String,
}

impl CaptureIssue {
    /// The affected repository-relative path, when one exists.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The kind of condition.
    #[must_use]
    pub const fn kind(&self) -> CaptureIssueKind {
        self.kind
    }

    /// The actionable explanation.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Why a capture needs to be explained or rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CaptureIssueKind {
    /// The path is outside the existing visible/ignore eligibility policy.
    Excluded,
    /// The path could not be read.
    Read,
    /// The path is binary, oversized, or has an unsupported type.
    Unsupported,
    /// The path changed while it was being captured.
    Race,
    /// A committed backing object is unavailable.
    MissingObject,
}

/// The result of an explicit capture attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureResult {
    point: Option<ReviewPoint>,
    issues: Vec<CaptureIssue>,
}

impl CaptureResult {
    /// The published point, or `None` when capture was not selectable.
    #[must_use]
    pub fn point(&self) -> Option<&ReviewPoint> {
        self.point.as_ref()
    }

    /// Conditions found while evaluating the workspace.
    #[must_use]
    pub fn issues(&self) -> &[CaptureIssue] {
        &self.issues
    }

    /// Whether a selectable point was durably published.
    #[must_use]
    pub const fn published(&self) -> bool {
        self.point.is_some()
    }
}

/// The result of deleting one review point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteResult {
    point: Option<ReviewPoint>,
    reclaimed_blobs: usize,
    reclaimed_bytes: u64,
    issues: Vec<DeleteIssue>,
}

impl DeleteResult {
    /// The point that was durably deleted, or `None` when it was already gone.
    #[must_use]
    pub fn point(&self) -> Option<&ReviewPoint> {
        self.point.as_ref()
    }

    /// Number of unshared blob files removed.
    #[must_use]
    pub const fn reclaimed_blobs(&self) -> usize {
        self.reclaimed_blobs
    }

    /// Bytes occupied by the removed blob files.
    #[must_use]
    pub const fn reclaimed_bytes(&self) -> u64 {
        self.reclaimed_bytes
    }

    /// Post-commit cleanup or lock-release failures.
    #[must_use]
    pub fn issues(&self) -> &[DeleteIssue] {
        &self.issues
    }
}

/// A failure after a review-point deletion was durably committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteIssue {
    blob: Option<String>,
    detail: String,
}

impl DeleteIssue {
    /// The affected content digest, when cleanup concerned one blob.
    #[must_use]
    pub fn blob(&self) -> Option<&str> {
        self.blob.as_deref()
    }

    /// The cleanup failure.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Persistent content-addressed review-point storage.
#[derive(Debug)]
pub struct ReviewPointStore {
    dir: PathBuf,
    points: BTreeMap<String, ReviewPoint>,
    log: File,
}

impl ReviewPointStore {
    /// Open the workspace's review points in a validated private XDG hierarchy.
    ///
    /// # Errors
    ///
    /// Returns an error for unsafe existing state or the errors of [`Self::open`].
    pub fn open_workspace(dirs: &crate::XdgDirs, key: &Path) -> Result<Self, ReviewPointError> {
        let directory = dirs.review_points_dir(key);
        dirs.prepare_state_dir(&directory)?;
        Self::open(directory)
    }

    /// Open or create a review-point store.
    ///
    /// The store directory and its blobs directory must be owned by the
    /// effective UID with mode 0700. Existing files must be singly linked
    /// regular files with mode 0600. Unsafe paths are refused, not repaired.
    ///
    /// # Errors
    ///
    /// Returns an I/O or format error when the store cannot be opened.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, ReviewPointError> {
        let dir = dir.as_ref().to_path_buf();
        crate::private_state::ensure_dir(&dir)?;
        crate::private_state::ensure_dir(dir.join(BLOB_DIR))?;
        let log_path = dir.join(INDEX_FILE);
        let mut log = crate::private_state::open_append(&log_path)?;
        let mut lock = FileLock::exclusive(&mut log)?;
        let result = replay_log(lock.file_mut());
        let unlock = lock.unlock();
        let points = match result {
            Ok(points) => {
                unlock?;
                points
            }
            Err(error) => {
                let _ = unlock;
                return Err(error);
            }
        };
        Ok(Self { dir, points, log })
    }

    /// Reload the active points from the durable manifest.
    ///
    /// An interrupted final record is discarded under the manifest lock.
    /// Complete malformed records still fail closed.
    ///
    /// # Errors
    ///
    /// Returns an I/O or format error without replacing the current map.
    pub fn reload(&mut self) -> Result<(), ReviewPointError> {
        let mut lock = FileLock::exclusive(&mut self.log)?;
        let result = replay_log(lock.file_mut());
        let unlock = lock.unlock();
        let points = match result {
            Ok(points) => {
                unlock?;
                points
            }
            Err(error) => {
                let _ = unlock;
                return Err(error);
            }
        };
        self.points = points;
        Ok(())
    }

    /// The storage directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// All points, oldest first.
    #[must_use]
    pub fn list(&self) -> Vec<ReviewPoint> {
        let mut points: Vec<_> = self.points.values().cloned().collect();
        points.sort_by(|left, right| {
            left.created
                .cmp(&right.created)
                .then_with(|| left.id.cmp(&right.id))
        });
        points
    }

    /// Find a point by its stable ID.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&ReviewPoint> {
        self.points.get(id)
    }

    /// Number of durable points.
    #[must_use]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Whether no review points are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Bytes occupied by content-addressed blobs.
    #[must_use]
    pub fn blob_bytes(&self) -> u64 {
        fs::read_dir(self.dir.join(BLOB_DIR)).map_or(0, |entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.metadata().ok())
                .map(|metadata| metadata.len())
                .sum()
        })
    }

    /// Delete one point and reclaim blobs no active point still references.
    ///
    /// The deletion record is made durable before any blob is removed.
    /// Cleanup failures are returned in [`DeleteResult`] because they do not
    /// undo the logical deletion.
    ///
    /// # Errors
    ///
    /// Returns an error when replay fails or the deletion commit cannot be
    /// established. [`ReviewPointError::CommitUncertain`] means callers must
    /// reload or retry before claiming whether deletion occurred.
    pub fn delete(&mut self, id: &str) -> Result<DeleteResult, ReviewPointError> {
        let event = StoredDeleteEvent {
            kind: DELETE_EVENT_NAME.to_owned(),
            id: id.to_owned(),
            deleted: crate::clock::now(),
        };
        let mut line = serde_json::to_vec(&event).map_err(|source| ReviewPointError::Json {
            detail: source.to_string(),
        })?;
        line.push(b'\n');
        let mut lock = FileLock::exclusive(&mut self.log)?;
        let result = delete_locked(&self.dir, lock.file_mut(), id, &line);
        match result {
            Ok((points, mut deleted)) => {
                if let Err(error) = lock.unlock() {
                    deleted.issues.push(DeleteIssue {
                        blob: None,
                        detail: format!("manifest lock release failed: {error}"),
                    });
                }
                self.points = points;
                Ok(deleted)
            }
            Err(error) => {
                let _ = lock.unlock();
                Err(error)
            }
        }
    }

    /// Rename one review point using its latest snapshot as a compare-and-swap token.
    ///
    /// Leading and trailing whitespace is removed from `replacement`; a blank
    /// replacement clears the name. Names are case-sensitive and must be unique
    /// among active review points.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewPointError::InvalidName`] for a name outside the
    /// single-line length policy, [`ReviewPointError::DuplicateName`] when an
    /// active point already uses the name, [`ReviewPointError::StaleRename`]
    /// when `expected` is not the latest renamed snapshot, or
    /// [`ReviewPointError::Unavailable`] when the point was deleted.
    /// [`ReviewPointError::CommitUncertain`] means callers must reload before
    /// deciding whether the rename occurred.
    pub fn rename(
        &mut self,
        expected: &ReviewPoint,
        replacement: Option<&str>,
    ) -> Result<ReviewPoint, ReviewPointError> {
        let replacement = normalize_name(replacement)?;
        let mut lock = FileLock::exclusive(&mut self.log)?;
        let result = rename_locked(&self.dir, lock.file_mut(), expected, replacement.as_deref());
        let unlock = lock.unlock();
        let (points, renamed) = match result {
            Ok(result) => {
                if let Err(error) = unlock {
                    tracing::warn!(%error, "review-point manifest lock release failed");
                }
                result
            }
            Err(error) => {
                let _ = unlock;
                return Err(error);
            }
        };
        self.points = points;
        Ok(renamed)
    }

    /// Capture an explicit workspace review point.
    ///
    /// The point records the observed `HEAD`, checkout identity, name, mode
    /// facts, deletions, and changed eligible bytes. Exclusions are retained
    /// on a published point; read, race, unsupported, and missing-object
    /// conditions return an unpublished result.
    ///
    /// # Errors
    ///
    /// Returns an error when workspace traversal is incomplete or the store
    /// cannot persist durable blobs and its manifest log.
    /// [`ReviewPointError::CommitUncertain`] means callers must reload before
    /// claiming whether the point was published.
    #[expect(
        clippy::too_many_lines,
        reason = "capture keeps the pre-publication manifest validation in one transaction"
    )]
    pub fn capture(
        &mut self,
        workspace: &mut Workspace,
        name: Option<&str>,
    ) -> Result<CaptureResult, ReviewPointError> {
        let name = normalize_name(name)?;
        let head = workspace
            .head_commit()
            .map(|hex| {
                CommitId::parse(hex).map_err(|error| ReviewPointError::Invalid(error.to_string()))
            })
            .transpose()?;
        let baseline_endpoint = head
            .clone()
            .map_or(ComparisonEndpoint::EmptyTree, ComparisonEndpoint::Commit);
        let baseline_files = workspace.endpoint_files(&baseline_endpoint)?;
        let current_files = workspace.endpoint_files(&ComparisonEndpoint::WorkingTree)?;
        let mut issues = excluded_paths(workspace, &current_files);
        let mut entries = Vec::new();
        let mut blobs = Vec::new();
        let mut paths = BTreeSet::new();
        paths.extend(baseline_files.keys().cloned());
        paths.extend(current_files.keys().cloned());

        for path in paths {
            let baseline_file = baseline_files.get(&path);
            let current_file = current_files.get(&path);
            match (baseline_file, current_file) {
                (Some(baseline), None) if !baseline.info.mode().is_directory() => {
                    entries.push(ReviewEntry {
                        path,
                        mode: baseline.info.mode(),
                        size: baseline.info.size(),
                        content: ReviewContent::Tombstone,
                    });
                }
                (Some(baseline), Some(current))
                    if baseline.info.mode().is_directory()
                        && current.info.mode().is_directory() => {}
                (Some(_baseline), Some(current)) if current.info.mode().is_directory() => {
                    issues.push(issue(
                        Some(path),
                        CaptureIssueKind::Unsupported,
                        "directory/file type changes are outside the eligible file capture policy",
                    ));
                }
                (None, Some(current)) if current.info.mode().is_directory() => {}
                (None | Some(_), None) => {}
                (None | Some(_), Some(current)) => {
                    if !current.info.is_supported() {
                        issues.push(issue(
                            Some(path),
                            CaptureIssueKind::Unsupported,
                            "path mode or size is outside the eligible capture policy",
                        ));
                        continue;
                    }
                    let Some(bytes) = capture_worktree_bytes(workspace, &path, &mut issues) else {
                        continue;
                    };
                    if bytes.len() as u64 > content::Policy::default().max_bytes {
                        issues.push(issue(
                            Some(path),
                            CaptureIssueKind::Unsupported,
                            "content exceeds the configured review-point size limit",
                        ));
                        continue;
                    }
                    let binary = workspace.diff_attr(&path).classify(&bytes).unwrap_or(false);
                    if binary {
                        if let Some(baseline) = baseline_file
                            && baseline.info.is_supported()
                            && workspace
                                .endpoint_bytes(&baseline_endpoint, &path)
                                .ok()
                                .flatten()
                                .is_some_and(|old| {
                                    old == bytes && baseline.info.mode() == current.info.mode()
                                })
                        {
                            continue;
                        }
                        issues.push(issue(
                            Some(path),
                            CaptureIssueKind::Unsupported,
                            "binary content is outside the eligible text capture policy",
                        ));
                        continue;
                    }
                    let unchanged = if let Some(baseline) = baseline_file {
                        if baseline.info.is_supported() {
                            match workspace.endpoint_bytes(&baseline_endpoint, &path) {
                                Ok(Some(old)) => {
                                    old == bytes && baseline.info.mode() == current.info.mode()
                                }
                                Ok(None) => false,
                                Err(error) => {
                                    issues.push(issue(
                                        Some(path.clone()),
                                        CaptureIssueKind::MissingObject,
                                        error.to_string(),
                                    ));
                                    false
                                }
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if unchanged {
                        continue;
                    }
                    let blob = hash(&bytes);
                    blobs.push((blob.clone(), bytes));
                    entries.push(ReviewEntry {
                        path,
                        mode: current.info.mode(),
                        size: current.info.size(),
                        content: ReviewContent::Blob(blob),
                    });
                }
            }
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        issues.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.kind.cmp(&right.kind))
        });

        let fatal = issues
            .iter()
            .any(|issue| issue.kind != CaptureIssueKind::Excluded);
        if fatal {
            return Ok(CaptureResult {
                point: None,
                issues,
            });
        }
        if workspace.head_commit() != head.as_ref().map(ToString::to_string) {
            let race = issue(
                None,
                CaptureIssueKind::Race,
                "HEAD changed while the review point was being captured",
            );
            issues.push(race.clone());
            return Ok(CaptureResult {
                point: None,
                issues,
            });
        }

        let created = crate::clock::now();
        let id = point_id(created, workspace, name.as_deref(), head.as_ref(), &entries);
        let point = ReviewPoint {
            id,
            name,
            name_revision: 0,
            created,
            checkout: workspace.root().to_path_buf(),
            workspace_key: workspace.key().to_path_buf(),
            head,
            entries,
            issues: issues.clone(),
        };
        self.publish(&point, &blobs)?;
        Ok(CaptureResult {
            point: Some(point),
            issues,
        })
    }

    /// Reconstruct one path from a saved review point.
    ///
    /// Unchanged paths are loaded from the point's observed commit. If that
    /// Git object was pruned, the error names the point and path instead of
    /// substituting current `HEAD` or working-tree content.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewPointError`] when the point or backing blob cannot be
    /// read.
    pub fn load_bytes(
        &self,
        point: &ReviewPoint,
        workspace: &Workspace,
        path: &Path,
    ) -> Result<Option<Vec<u8>>, ReviewPointError> {
        self.require_active(point)?;
        if let Some(entry) = point.entry(path) {
            return match &entry.content {
                ReviewContent::Tombstone => Ok(None),
                ReviewContent::Blob(blob) => self.read_blob(blob, point, path, workspace).map(Some),
            };
        }
        if let Some(issue) = point
            .issues
            .iter()
            .find(|issue| issue.path.as_deref() == Some(path))
        {
            return Err(ReviewPointError::MissingBacking {
                point: point.id.clone(),
                path: path.to_path_buf(),
                detail: issue.detail.clone(),
            });
        }
        let Some(head) = point.head() else {
            return Ok(None);
        };
        workspace
            .endpoint_bytes(&ComparisonEndpoint::Commit(head.clone()), path)
            .map_err(|error| ReviewPointError::MissingBacking {
                point: point.id.clone(),
                path: path.to_path_buf(),
                detail: error.to_string(),
            })
    }

    /// Compare a saved point directly with the current working tree.
    ///
    /// The point is an immutable base. The target is the final working-tree
    /// state, so staged and unstaged layers are not concatenated.
    ///
    /// # Errors
    ///
    /// Returns [`ReviewPointError`] when a point blob or its committed
    /// baseline cannot be reconstructed.
    pub fn compare_to_working(
        &self,
        point: &ReviewPoint,
        workspace: &mut Workspace,
    ) -> Result<Comparison, ReviewPointError> {
        self.require_active(point)?;
        workspace.begin_comparison();
        let baseline = point
            .head()
            .cloned()
            .map_or(ComparisonEndpoint::EmptyTree, ComparisonEndpoint::Commit);
        let mut point_files = workspace.endpoint_files(&baseline)?;
        for entry in &point.entries {
            workspace.check_path_count(point_files.len().saturating_add(1))?;
            if entry.is_tombstone() {
                point_files.remove(&entry.path);
            } else {
                let blob = entry.blob().ok_or_else(|| {
                    ReviewPointError::Invalid("review-point blob entry has no digest".to_owned())
                })?;
                point_files.insert(
                    entry.path.clone(),
                    EndpointFile {
                        info: PathInfo::new(entry.mode, entry.size, Some(blob.to_owned()), false),
                    },
                );
            }
        }
        let working_files = workspace.endpoint_files(&ComparisonEndpoint::WorkingTree)?;
        let target_paths = working_files
            .iter()
            .filter(|(_, file)| !file.info.mode().is_directory())
            .map(|(path, _)| path.clone())
            .collect();
        let mut paths = BTreeSet::new();
        paths.extend(point_files.keys().cloned());
        paths.extend(working_files.keys().cloned());
        workspace.check_path_count(paths.len())?;
        let mut changes = Vec::new();
        for path in paths {
            workspace.check_scan()?;
            let point_file = point_files.get(&path);
            let working_file = working_files.get(&path);
            let mut base_state = point_file.map_or(PathState::Absent, |file| {
                PathState::Present(file.info.clone())
            });
            let mut target_state = working_file.map_or(PathState::Absent, |file| {
                PathState::Present(file.info.clone())
            });
            let base_bytes = if let Some(file) = point_file {
                if file.info.is_supported() {
                    let bytes = self.load_bytes(point, workspace, &path)?;
                    if let Some(bytes) = bytes.as_deref()
                        && let PathState::Present(info) = &mut base_state
                    {
                        *info = info.clone().with_binary(
                            workspace.diff_attr(&path).classify(bytes).unwrap_or(false),
                        );
                    }
                    bytes
                } else {
                    None
                }
            } else {
                None
            };
            let target_bytes = if working_file.is_some_and(|file| file.info.is_supported()) {
                let bytes = workspace.endpoint_bytes(&ComparisonEndpoint::WorkingTree, &path)?;
                if let Some(bytes) = bytes.as_deref()
                    && let PathState::Present(info) = &mut target_state
                {
                    *info = info
                        .clone()
                        .with_binary(workspace.diff_attr(&path).classify(bytes).unwrap_or(false));
                }
                bytes
            } else {
                None
            };
            if let Some(change) = PathChange::from_states(
                path,
                base_state,
                target_state,
                base_bytes.as_deref(),
                target_bytes.as_deref(),
            ) {
                changes.push(change);
            }
        }
        Ok(Comparison::from_parts(
            ComparisonEndpoint::ReviewPoint(point.id.clone()),
            ComparisonEndpoint::WorkingTree,
            target_paths,
            changes,
        ))
    }

    fn read_blob(
        &self,
        blob: &str,
        point: &ReviewPoint,
        path: &Path,
        workspace: &Workspace,
    ) -> Result<Vec<u8>, ReviewPointError> {
        workspace.check_scan()?;
        let mut bytes = Vec::new();
        crate::private_state::open_read(self.dir.join(BLOB_DIR).join(blob))
            .and_then(|file| {
                file.take(workspace.content_limit().saturating_add(1))
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| ReviewPointError::MissingBacking {
                point: point.id.clone(),
                path: path.to_path_buf(),
                detail: format!("content blob {blob} is unavailable: {error}"),
            })?;
        workspace.charge_content(bytes.len())?;
        Ok(bytes)
    }

    fn require_active(&self, point: &ReviewPoint) -> Result<(), ReviewPointError> {
        if self.points.contains_key(point.id()) {
            Ok(())
        } else {
            Err(ReviewPointError::Unavailable {
                point: point.id.clone(),
            })
        }
    }

    fn publish(
        &mut self,
        point: &ReviewPoint,
        blobs: &[(String, Vec<u8>)],
    ) -> Result<(), ReviewPointError> {
        let event = StoredPointEvent::from_point(point);
        let mut line = serde_json::to_vec(&event).map_err(|source| ReviewPointError::Json {
            detail: source.to_string(),
        })?;
        line.push(b'\n');
        let mut lock = FileLock::exclusive(&mut self.log)?;
        let result = publish_locked(&self.dir, lock.file_mut(), point, blobs, &line);
        let unlock = lock.unlock();
        let points = match result {
            Ok(points) => {
                if let Err(error) = unlock {
                    tracing::warn!(%error, "review-point manifest lock release failed");
                }
                points
            }
            Err(error) => {
                let _ = unlock;
                return Err(error);
            }
        };
        self.points = points;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReviewContent {
    Blob(String),
    Tombstone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPointEvent {
    #[serde(rename = "event")]
    kind: String,
    id: String,
    name: Option<String>,
    created: u64,
    checkout: PathBuf,
    workspace_key: PathBuf,
    head: Option<String>,
    entries: Vec<StoredEntry>,
    issues: Vec<StoredIssue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredDeleteEvent {
    #[serde(rename = "event")]
    kind: String,
    id: String,
    deleted: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredRenameEvent {
    #[serde(rename = "event")]
    kind: String,
    id: String,
    previous: Option<String>,
    replacement: Option<String>,
    revision: u64,
    renamed: u64,
}

#[derive(Debug, Deserialize)]
struct StoredEventKind {
    #[serde(rename = "event")]
    kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredEntry {
    path: PathBuf,
    mode: u32,
    size: Option<u64>,
    blob: Option<String>,
    tombstone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredIssue {
    path: Option<PathBuf>,
    kind: CaptureIssueKind,
    detail: String,
}

impl StoredPointEvent {
    fn from_point(point: &ReviewPoint) -> Self {
        Self {
            kind: EVENT_NAME.to_owned(),
            id: point.id.clone(),
            name: point.name.clone(),
            created: point.created,
            checkout: point.checkout.clone(),
            workspace_key: point.workspace_key.clone(),
            head: point.head.as_ref().map(ToString::to_string),
            entries: point
                .entries
                .iter()
                .map(|entry| StoredEntry {
                    path: entry.path.clone(),
                    mode: raw_mode(entry.mode),
                    size: entry.size,
                    blob: entry.blob().map(str::to_owned),
                    tombstone: entry.is_tombstone(),
                })
                .collect(),
            issues: point
                .issues
                .iter()
                .map(|issue| StoredIssue {
                    path: issue.path.clone(),
                    kind: issue.kind,
                    detail: issue.detail.clone(),
                })
                .collect(),
        }
    }

    fn into_point(self) -> Result<ReviewPoint, ReviewPointError> {
        if !is_digest(&self.id) {
            return Err(ReviewPointError::Invalid(
                "review-point id must be a lowercase SHA-256 digest".to_owned(),
            ));
        }
        let head = self
            .head
            .map(CommitId::parse)
            .transpose()
            .map_err(|error| ReviewPointError::Invalid(error.to_string()))?;
        let mut entries = self
            .entries
            .into_iter()
            .map(|entry| {
                if entry.tombstone == entry.blob.is_some() {
                    return Err(ReviewPointError::Invalid(
                        "review-point entry must be either a blob or tombstone".to_owned(),
                    ));
                }
                if let Some(blob) = entry.blob.as_deref()
                    && !is_digest(blob)
                {
                    return Err(ReviewPointError::Invalid(
                        "review-point blob must be a lowercase SHA-256 digest".to_owned(),
                    ));
                }
                Ok(ReviewEntry {
                    path: entry.path,
                    mode: mode_from_raw(entry.mode),
                    size: entry.size,
                    content: if entry.tombstone {
                        ReviewContent::Tombstone
                    } else {
                        ReviewContent::Blob(entry.blob.ok_or_else(|| {
                            ReviewPointError::Invalid(
                                "blob entry lost its digest during validation".to_owned(),
                            )
                        })?)
                    },
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(ReviewPoint {
            id: self.id,
            name: self.name,
            name_revision: 0,
            created: self.created,
            checkout: self.checkout,
            workspace_key: self.workspace_key,
            head,
            entries,
            issues: self
                .issues
                .into_iter()
                .map(|issue| CaptureIssue {
                    path: issue.path,
                    kind: issue.kind,
                    detail: issue.detail,
                })
                .collect(),
        })
    }
}

impl Serialize for CaptureIssueKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            Self::Excluded => "excluded",
            Self::Read => "read",
            Self::Unsupported => "unsupported",
            Self::Race => "race",
            Self::MissingObject => "missing-object",
        })
    }
}

impl<'de> Deserialize<'de> for CaptureIssueKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "excluded" => Ok(Self::Excluded),
            "read" => Ok(Self::Read),
            "unsupported" => Ok(Self::Unsupported),
            "race" => Ok(Self::Race),
            "missing-object" => Ok(Self::MissingObject),
            _ => Err(serde::de::Error::custom("unknown review-point issue kind")),
        }
    }
}

/// Errors while opening, publishing, or reconstructing review points.
#[derive(Debug, Error)]
pub enum ReviewPointError {
    /// The review-point directory or log could not be read or written.
    #[error("review-point storage I/O failed: {0}")]
    Io(#[from] io::Error),
    /// A manifest log line was malformed.
    #[error("review-point line {line} is malformed: {detail}")]
    Format {
        /// The one-based line number in the manifest log.
        line: usize,
        /// The owned parser error detail.
        detail: String,
    },
    /// A log line used an event kind this build does not understand.
    #[error("review-point line {line} has unexpected event `{event}`")]
    UnexpectedEvent {
        /// The one-based line number in the manifest log.
        line: usize,
        /// The unsupported event name.
        event: String,
    },
    /// A workspace read failed.
    #[error("workspace capture failed: {0}")]
    Workspace(#[from] WorkspaceError),
    /// A review point or manifest entry is invalid.
    #[error("invalid review point: {0}")]
    Invalid(String),
    /// A proposed name violates the single-line length policy.
    #[error("review-point name must be a single line of at most 128 characters")]
    InvalidName,
    /// A proposed name is already used by an active point.
    #[error("review-point name is already in use")]
    DuplicateName,
    /// A rename used a snapshot that is no longer current.
    #[error("review point {point} was renamed since this snapshot was loaded")]
    StaleRename {
        /// The point whose snapshot is stale.
        point: String,
    },
    /// A saved point's required content is unavailable.
    #[error("review point {point} cannot reconstruct {}: {detail}", path.display())]
    MissingBacking {
        /// The point whose content is unavailable.
        point: String,
        /// The requested repository-relative path.
        path: PathBuf,
        /// The backing failure.
        detail: String,
    },
    /// A point has been logically deleted or is otherwise no longer active.
    #[error("review point {point} is no longer available")]
    Unavailable {
        /// The inactive point identifier.
        point: String,
    },
    /// A manifest write failed after its durable outcome became uncertain.
    #[error("review point {point} update outcome is uncertain: {detail}")]
    CommitUncertain {
        /// The point whose update must be reloaded before retrying.
        point: String,
        /// The underlying append, flush, or sync failure.
        detail: String,
    },
    /// JSON serialization failed before publication.
    #[error("cannot serialize review point: {detail}")]
    Json {
        /// The owned serializer error detail.
        detail: String,
    },
}

fn excluded_paths(
    workspace: &mut Workspace,
    current_files: &BTreeMap<PathBuf, EndpointFile>,
) -> Vec<CaptureIssue> {
    workspace
        .walk_files(Filter::All)
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| !current_files.contains_key(path))
        .filter(|path| workspace.is_ignored(path, crate::workspace::EntryKind::File))
        .map(|path| {
            issue(
                Some(path),
                CaptureIssueKind::Excluded,
                "ignored by the workspace eligibility policy",
            )
        })
        .collect()
}

fn capture_worktree_bytes(
    workspace: &Workspace,
    path: &Path,
    issues: &mut Vec<CaptureIssue>,
) -> Option<Vec<u8>> {
    let absolute = workspace.root().join(path);
    let before = match fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            issues.push(issue(
                Some(path.to_path_buf()),
                CaptureIssueKind::Race,
                "path disappeared before it could be captured",
            ));
            return None;
        }
        Err(error) => {
            issues.push(issue(
                Some(path.to_path_buf()),
                CaptureIssueKind::Read,
                format!("cannot inspect path: {error}"),
            ));
            return None;
        }
    };
    let bytes = match workspace.endpoint_bytes(&ComparisonEndpoint::WorkingTree, path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            issues.push(issue(
                Some(path.to_path_buf()),
                CaptureIssueKind::Race,
                "path disappeared while it was being captured",
            ));
            return None;
        }
        Err(error) => {
            issues.push(issue(
                Some(path.to_path_buf()),
                CaptureIssueKind::Read,
                error.to_string(),
            ));
            return None;
        }
    };
    let after = match fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) => {
            issues.push(issue(
                Some(path.to_path_buf()),
                CaptureIssueKind::Race,
                format!("cannot verify path after reading: {error}"),
            ));
            return None;
        }
    };
    if fingerprint(&before) != fingerprint(&after) {
        issues.push(issue(
            Some(path.to_path_buf()),
            CaptureIssueKind::Race,
            "path metadata changed while it was being captured",
        ));
        return None;
    }
    Some(bytes)
}

fn fingerprint(metadata: &fs::Metadata) -> (u64, bool, bool, Option<u128>) {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    (
        metadata.len(),
        metadata.is_file(),
        metadata.is_symlink(),
        modified,
    )
}

fn issue(path: Option<PathBuf>, kind: CaptureIssueKind, detail: impl Into<String>) -> CaptureIssue {
    CaptureIssue {
        path,
        kind,
        detail: detail.into(),
    }
}

fn normalize_name(name: Option<&str>) -> Result<Option<String>, ReviewPointError> {
    let Some(raw_name) = name else {
        return Ok(None);
    };
    let name = raw_name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    let mut scalars = 0;
    for character in name.chars() {
        scalars += 1;
        if scalars > MAX_NAME_SCALARS
            || character.is_control()
            || matches!(character, '\u{2028}' | '\u{2029}')
        {
            return Err(ReviewPointError::InvalidName);
        }
    }
    Ok(Some(name.to_owned()))
}

fn ensure_name_available(
    points: &BTreeMap<String, ReviewPoint>,
    name: Option<&str>,
    except_id: Option<&str>,
) -> Result<(), ReviewPointError> {
    let Some(name) = name else {
        return Ok(());
    };
    if points
        .values()
        .any(|point| Some(point.id()) != except_id && point.name() == Some(name))
    {
        return Err(ReviewPointError::DuplicateName);
    }
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn point_id(
    created: u64,
    workspace: &Workspace,
    name: Option<&str>,
    head: Option<&CommitId>,
    entries: &[ReviewEntry],
) -> String {
    let mut bytes = created.to_le_bytes().to_vec();
    bytes.extend_from_slice(nonce().to_string().as_bytes());
    bytes.extend_from_slice(workspace.root().as_os_str().as_encoded_bytes());
    bytes.extend_from_slice(workspace.key().as_os_str().as_encoded_bytes());
    if let Some(name) = name {
        bytes.extend_from_slice(name.as_bytes());
    }
    if let Some(head) = head {
        bytes.extend_from_slice(head.as_str().as_bytes());
    }
    for entry in entries {
        bytes.extend_from_slice(entry.path.as_os_str().as_encoded_bytes());
        bytes.extend_from_slice(&raw_mode(entry.mode).to_le_bytes());
        if let Some(blob) = entry.blob() {
            bytes.extend_from_slice(blob.as_bytes());
        }
    }
    hash(&bytes)
}

fn nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

fn raw_mode(mode: FileMode) -> u32 {
    match mode {
        FileMode::Regular => 0o100_644,
        FileMode::Executable => 0o100_755,
        FileMode::Symlink => 0o120_000,
        FileMode::Directory => 0o040_000,
        FileMode::Submodule => 0o160_000,
        FileMode::Other(value) => value,
    }
}

fn mode_from_raw(mode: u32) -> FileMode {
    match mode {
        0o100_644 => FileMode::Regular,
        0o100_755 => FileMode::Executable,
        0o120_000 => FileMode::Symlink,
        0o040_000 => FileMode::Directory,
        0o160_000 => FileMode::Submodule,
        other => FileMode::Other(other),
    }
}

struct FileLock<'a> {
    file: &'a mut File,
    locked: bool,
}

impl<'a> FileLock<'a> {
    fn exclusive(file: &'a mut File) -> io::Result<Self> {
        file.lock()?;
        Ok(Self { file, locked: true })
    }

    fn file_mut(&mut self) -> &mut File {
        self.file
    }

    fn unlock(mut self) -> io::Result<()> {
        let result = self.file.unlock();
        self.locked = false;
        result
    }
}

impl Drop for FileLock<'_> {
    fn drop(&mut self) {
        if self.locked {
            let _ = self.file.unlock();
        }
    }
}

fn replay_log(file: &mut File) -> Result<BTreeMap<String, ReviewPoint>, ReviewPointError> {
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let complete = if bytes.last() == Some(&b'\n') {
        bytes.len()
    } else {
        bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1)
    };
    if complete < bytes.len() {
        file.set_len(u64::try_from(complete).map_err(io::Error::other)?)?;
        bytes.truncate(complete);
    }
    file.sync_all()?;
    let text = std::str::from_utf8(&bytes).map_err(|source| ReviewPointError::Format {
        line: bytes[..source.valid_up_to()]
            .split(|byte| *byte == b'\n')
            .count(),
        detail: source.to_string(),
    })?;
    let mut points = BTreeMap::new();
    for (line_number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let kind: StoredEventKind =
            serde_json::from_str(line).map_err(|source| ReviewPointError::Format {
                line: line_number + 1,
                detail: source.to_string(),
            })?;
        match kind.kind.as_str() {
            EVENT_NAME => {
                let event: StoredPointEvent =
                    serde_json::from_str(line).map_err(|source| ReviewPointError::Format {
                        line: line_number + 1,
                        detail: source.to_string(),
                    })?;
                let point = event.into_point()?;
                points.insert(point.id.clone(), point);
            }
            DELETE_EVENT_NAME => {
                let event: StoredDeleteEvent =
                    serde_json::from_str(line).map_err(|source| ReviewPointError::Format {
                        line: line_number + 1,
                        detail: source.to_string(),
                    })?;
                if !is_digest(&event.id) {
                    return Err(ReviewPointError::Invalid(
                        "deleted review-point id must be a lowercase SHA-256 digest".to_owned(),
                    ));
                }
                let _ = event.deleted;
                points.remove(&event.id);
            }
            RENAME_EVENT_NAME => {
                let event: StoredRenameEvent =
                    serde_json::from_str(line).map_err(|source| ReviewPointError::Format {
                        line: line_number + 1,
                        detail: source.to_string(),
                    })?;
                apply_rename_event(&mut points, &event)?;
            }
            event => {
                return Err(ReviewPointError::UnexpectedEvent {
                    line: line_number + 1,
                    event: event.to_owned(),
                });
            }
        }
    }
    Ok(points)
}

fn apply_rename_event(
    points: &mut BTreeMap<String, ReviewPoint>,
    event: &StoredRenameEvent,
) -> Result<(), ReviewPointError> {
    if !is_digest(&event.id) {
        return Err(ReviewPointError::Invalid(
            "renamed review-point id must be a lowercase SHA-256 digest".to_owned(),
        ));
    }
    let replacement = normalize_name(event.replacement.as_deref())?;
    if replacement.as_ref() != event.replacement.as_ref() {
        return Err(ReviewPointError::InvalidName);
    }
    let point = points.get(&event.id).ok_or_else(|| {
        ReviewPointError::Invalid("review-point rename target is unavailable".to_owned())
    })?;
    if point.name.as_ref() != event.previous.as_ref() {
        return Err(ReviewPointError::Invalid(
            "review-point rename previous name does not match".to_owned(),
        ));
    }
    let revision = point.name_revision.checked_add(1).ok_or_else(|| {
        ReviewPointError::Invalid("review-point name revision overflowed".to_owned())
    })?;
    if revision != event.revision {
        return Err(ReviewPointError::Invalid(
            "review-point rename revision is not monotonic".to_owned(),
        ));
    }
    ensure_name_available(points, replacement.as_deref(), Some(&event.id))?;
    let current = points.get_mut(&event.id).ok_or_else(|| {
        ReviewPointError::Invalid("review-point rename target is unavailable".to_owned())
    })?;
    current.name = replacement;
    current.name_revision = revision;
    let _ = event.renamed;
    Ok(())
}

fn delete_locked(
    dir: &Path,
    file: &mut File,
    id: &str,
    line: &[u8],
) -> Result<(BTreeMap<String, ReviewPoint>, DeleteResult), ReviewPointError> {
    let mut points = replay_log(file)?;
    let Some(point) = points.get(id).cloned() else {
        return Ok((
            points,
            DeleteResult {
                point: None,
                reclaimed_blobs: 0,
                reclaimed_bytes: 0,
                issues: Vec::new(),
            },
        ));
    };
    let candidates: BTreeSet<String> = point
        .entries()
        .iter()
        .filter_map(ReviewEntry::blob)
        .map(str::to_owned)
        .collect();

    file.seek(SeekFrom::End(0))?;
    if let Err(error) = file
        .write_all(line)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
    {
        return Err(ReviewPointError::CommitUncertain {
            point: id.to_owned(),
            detail: error.to_string(),
        });
    }
    points.remove(id);

    let referenced: BTreeSet<&str> = points
        .values()
        .flat_map(ReviewPoint::entries)
        .filter_map(ReviewEntry::blob)
        .collect();
    let blob_dir = dir.join(BLOB_DIR);
    let mut reclaimed_blobs = 0;
    let mut reclaimed_bytes = 0_u64;
    let mut issues = Vec::new();
    for blob in candidates {
        if referenced.contains(blob.as_str()) {
            continue;
        }
        let path = blob_dir.join(&blob);
        let opened = match crate::private_state::open_read(&path) {
            Ok(opened) => opened,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                issues.push(DeleteIssue {
                    blob: Some(blob),
                    detail: error.to_string(),
                });
                continue;
            }
        };
        let size = match opened.metadata() {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                issues.push(DeleteIssue {
                    blob: Some(blob),
                    detail: error.to_string(),
                });
                continue;
            }
        };
        drop(opened);
        match fs::remove_file(&path) {
            Ok(()) => {
                reclaimed_blobs += 1;
                reclaimed_bytes = reclaimed_bytes.saturating_add(size);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => issues.push(DeleteIssue {
                blob: Some(blob),
                detail: error.to_string(),
            }),
        }
    }
    if reclaimed_blobs > 0
        && let Err(error) = sync_directory(&blob_dir)
    {
        issues.push(DeleteIssue {
            blob: None,
            detail: format!("blob directory sync failed: {error}"),
        });
    }
    Ok((
        points,
        DeleteResult {
            point: Some(point),
            reclaimed_blobs,
            reclaimed_bytes,
            issues,
        },
    ))
}

fn rename_locked(
    dir: &Path,
    file: &mut File,
    expected: &ReviewPoint,
    replacement: Option<&str>,
) -> Result<(BTreeMap<String, ReviewPoint>, ReviewPoint), ReviewPointError> {
    let mut points = replay_log(file)?;
    let current = points
        .get(expected.id())
        .ok_or_else(|| ReviewPointError::Unavailable {
            point: expected.id.clone(),
        })?
        .clone();
    if current.name_revision != expected.name_revision {
        return Err(ReviewPointError::StaleRename {
            point: expected.id.clone(),
        });
    }
    if current.name() == replacement {
        return Ok((points, current));
    }
    ensure_name_available(&points, replacement, Some(current.id()))?;
    let revision = current.name_revision.checked_add(1).ok_or_else(|| {
        ReviewPointError::Invalid("review-point name revision overflowed".to_owned())
    })?;
    let event = StoredRenameEvent {
        kind: RENAME_EVENT_NAME.to_owned(),
        id: current.id.clone(),
        previous: current.name.clone(),
        replacement: replacement.map(str::to_owned),
        revision,
        renamed: crate::clock::now(),
    };
    let mut line = serde_json::to_vec(&event).map_err(|source| ReviewPointError::Json {
        detail: source.to_string(),
    })?;
    line.push(b'\n');
    file.seek(SeekFrom::End(0))?;
    if let Err(error) = file
        .write_all(&line)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .and_then(|()| sync_directory(dir))
    {
        return Err(ReviewPointError::CommitUncertain {
            point: expected.id.clone(),
            detail: error.to_string(),
        });
    }
    let renamed = points
        .get_mut(expected.id())
        .ok_or_else(|| ReviewPointError::Unavailable {
            point: expected.id.clone(),
        })?;
    renamed.name = replacement.map(str::to_owned);
    renamed.name_revision = revision;
    let renamed = renamed.clone();
    Ok((points, renamed))
}

fn publish_locked(
    dir: &Path,
    file: &mut File,
    point: &ReviewPoint,
    blobs: &[(String, Vec<u8>)],
    line: &[u8],
) -> Result<BTreeMap<String, ReviewPoint>, ReviewPointError> {
    let mut points = replay_log(file)?;
    ensure_name_available(&points, point.name(), None)?;
    for (blob, bytes) in blobs {
        ensure_blob(dir, blob, bytes)?;
    }
    file.seek(SeekFrom::End(0))?;
    if let Err(error) = file
        .write_all(line)
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .and_then(|()| sync_directory(dir))
    {
        return Err(ReviewPointError::CommitUncertain {
            point: point.id.clone(),
            detail: error.to_string(),
        });
    }
    points.insert(point.id.clone(), point.clone());
    Ok(points)
}

fn ensure_blob(dir: &Path, blob: &str, bytes: &[u8]) -> io::Result<()> {
    let directory = dir.join(BLOB_DIR);
    let target = directory.join(blob);
    match crate::private_state::open_read(&target) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let temporary = directory.join(format!(".{blob}.{}.tmp", nonce()));
    let mut file = crate::private_state::create_new(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, &target)?;
        sync_directory(&directory)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use fathomable_testing::TempDir;

    use super::*;
    use fathomable_testing::git::{commit_and_stage, init};

    #[test]
    fn review_points_deduplicate_bytes_and_reconstruct_saved_untracked_edits()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-dedup")?;
        init(&dir.0)?;
        commit_and_stage(&dir.0, &[("tracked.md", "base\n")])?;
        fs::write(dir.0.join("added.md"), "same\n")?;
        let mut workspace = Workspace::discover(&dir.0)?;
        let mut store = ReviewPointStore::open(dir.0.join("state"))?;
        let first = store.capture(&mut workspace, Some("first"))?;
        let point = first
            .point()
            .ok_or("first point was not published")?
            .clone();

        fs::write(dir.0.join("added.md"), "changed\n")?;
        let second = store.capture(&mut workspace, Some("second"))?;
        let second_point = second
            .point()
            .ok_or("second point was not published")?
            .clone();
        assert_eq!(store.list().len(), 2);
        assert!(store.blob_bytes() >= b"same\n".len() as u64 + b"changed\n".len() as u64);

        fs::write(dir.0.join("added.md"), "later\n")?;
        assert_eq!(
            store.load_bytes(&point, &workspace, Path::new("added.md"))?,
            Some(b"same\n".to_vec())
        );
        assert_eq!(
            store.load_bytes(&second_point, &workspace, Path::new("added.md"))?,
            Some(b"changed\n".to_vec())
        );
        Ok(())
    }

    #[test]
    fn deleted_paths_are_tombstones_and_reopen_replays_points()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-tombstone")?;
        init(&dir.0)?;
        commit_and_stage(&dir.0, &[("gone.md", "gone\n"), ("kept.md", "kept\n")])?;
        fs::write(dir.0.join("gone.md"), "gone\n")?;
        fs::write(dir.0.join("kept.md"), "kept\n")?;
        fs::remove_file(dir.0.join("gone.md"))?;
        let mut workspace = Workspace::discover(&dir.0)?;
        let state = dir.0.join("state");
        let mut store = ReviewPointStore::open(&state)?;
        let result = store.capture(&mut workspace, None)?;
        let point = result.point().ok_or("point was not published")?.clone();
        let entry = point
            .entries()
            .iter()
            .find(|entry| entry.path() == Path::new("gone.md"))
            .ok_or("missing tombstone")?;
        assert!(entry.is_tombstone());
        assert_eq!(
            store.load_bytes(&point, &workspace, Path::new("gone.md"))?,
            None
        );

        drop(store);
        let store = ReviewPointStore::open(state)?;
        assert_eq!(store.get(point.id()).map(ReviewPoint::id), Some(point.id()));
        Ok(())
    }

    #[test]
    fn invalid_capture_is_reported_without_publication() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-invalid")?;
        init(&dir.0)?;
        commit_and_stage(&dir.0, &[("tracked.md", "base\n")])?;
        fs::write(dir.0.join("binary.bin"), b"a\0b")?;
        let mut workspace = Workspace::discover(&dir.0)?;
        let mut store = ReviewPointStore::open(dir.0.join("state"))?;
        let result = store.capture(&mut workspace, Some("invalid"))?;
        assert!(!result.published());
        assert!(
            result
                .issues()
                .iter()
                .any(|issue| issue.kind() == CaptureIssueKind::Unsupported)
        );
        assert_eq!(store.len(), 0);
        Ok(())
    }

    #[test]
    fn names_normalize_and_a_fresh_noop_does_not_append() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = TempDir::new("review-point-name-normalization")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let state = dir.0.join("state");
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(&state)?;

        let unnamed = store
            .capture(&mut workspace, Some(" \t "))?
            .point()
            .ok_or("unnamed point")?
            .clone();
        assert_eq!(unnamed.name(), None);
        let named = store
            .capture(&mut workspace, Some("  alpha  "))?
            .point()
            .ok_or("named point")?
            .clone();
        assert_eq!(named.name(), Some("alpha"));

        let manifest = state.join(INDEX_FILE);
        let before = fs::read(&manifest)?;
        let unchanged = store.rename(&named, Some(" alpha "))?;
        assert_eq!(unchanged, named);
        assert_eq!(fs::read(&manifest)?, before);

        let cleared = store.rename(&unchanged, Some("   "))?;
        assert_eq!(cleared.name(), None);
        assert_eq!(
            ReviewPointStore::open(state)?
                .get(cleared.id())
                .and_then(ReviewPoint::name),
            None
        );
        Ok(())
    }

    #[test]
    fn active_names_are_exact_case_sensitive_and_unique() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = TempDir::new("review-point-name-uniqueness")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(dir.0.join("state"))?;

        let lower = store
            .capture(&mut workspace, Some("alpha"))?
            .point()
            .ok_or("lowercase point")?
            .clone();
        let upper = store
            .capture(&mut workspace, Some("Alpha"))?
            .point()
            .ok_or("uppercase point")?
            .clone();
        assert!(matches!(
            store.capture(&mut workspace, Some(" alpha ")),
            Err(ReviewPointError::DuplicateName)
        ));
        assert!(matches!(
            store.rename(&upper, Some("alpha")),
            Err(ReviewPointError::DuplicateName)
        ));
        let renamed = store.rename(&upper, Some("ALPHA"))?;
        assert_eq!(renamed.name(), Some("ALPHA"));
        assert_eq!(
            store.get(lower.id()).and_then(ReviewPoint::name),
            Some("alpha")
        );
        Ok(())
    }

    #[test]
    fn new_names_enforce_scalar_and_single_line_limits() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-name-limits")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(dir.0.join("state"))?;
        let accepted_capture = "é".repeat(128);
        let point = store
            .capture(&mut workspace, Some(&accepted_capture))?
            .point()
            .ok_or("128-scalar capture")?
            .clone();
        assert_eq!(point.name(), Some(accepted_capture.as_str()));

        let accepted_rename = "界".repeat(128);
        let current = store.rename(&point, Some(&accepted_rename))?;
        assert_eq!(current.name(), Some(accepted_rename.as_str()));
        let rejected = [
            "x".repeat(129),
            "control\u{7f}name".to_owned(),
            "line\u{2028}separator".to_owned(),
            "paragraph\u{2029}separator".to_owned(),
        ];
        for name in &rejected {
            assert!(matches!(
                store.capture(&mut workspace, Some(name)),
                Err(ReviewPointError::InvalidName)
            ));
            assert!(matches!(
                store.rename(&current, Some(name)),
                Err(ReviewPointError::InvalidName)
            ));
        }
        Ok(())
    }

    #[test]
    fn rename_replays_without_changing_captured_state_then_deletes()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-rename-replay")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let state = dir.0.join("state");
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(&state)?;
        let original = store
            .capture(&mut workspace, Some("before"))?
            .point()
            .ok_or("point")?
            .clone();

        let renamed = store.rename(&original, Some("after"))?;
        assert_eq!(renamed.name(), Some("after"));
        assert_eq!(renamed.id(), original.id());
        assert_eq!(renamed.created(), original.created());
        assert_eq!(renamed.checkout(), original.checkout());
        assert_eq!(renamed.workspace_key(), original.workspace_key());
        assert_eq!(renamed.head(), original.head());
        assert_eq!(renamed.entries(), original.entries());
        assert_eq!(renamed.issues(), original.issues());
        assert_eq!(
            store.load_bytes(&renamed, &workspace, Path::new("a.md"))?,
            Some(b"point\n".to_vec())
        );

        let reopened = ReviewPointStore::open(&state)?;
        assert_eq!(
            reopened.get(original.id()).and_then(ReviewPoint::name),
            Some("after")
        );
        assert!(store.delete(original.id())?.point().is_some());
        assert!(matches!(
            store.rename(&renamed, Some("later")),
            Err(ReviewPointError::Unavailable { .. })
        ));
        assert!(ReviewPointStore::open(state)?.get(original.id()).is_none());
        Ok(())
    }

    #[test]
    fn rename_compare_and_swap_detects_stale_and_aba_snapshots()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-rename-cas")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let state = dir.0.join("state");
        let mut workspace = Workspace::discover(&repo)?;
        let mut first = ReviewPointStore::open(&state)?;
        let named = first
            .capture(&mut workspace, Some("A"))?
            .point()
            .ok_or("named point")?
            .clone();
        let mut second = ReviewPointStore::open(&state)?;
        let stale_named = second.get(named.id()).ok_or("second snapshot")?.clone();

        let named_b = first.rename(&named, Some("B"))?;
        assert!(matches!(
            second.rename(&stale_named, Some("C")),
            Err(ReviewPointError::StaleRename { .. })
        ));
        let named_a = first.rename(&named_b, Some("A"))?;
        assert_eq!(named_a.name(), Some("A"));
        assert!(matches!(
            second.rename(&stale_named, Some("A")),
            Err(ReviewPointError::StaleRename { .. })
        ));

        let unnamed = first
            .capture(&mut workspace, None)?
            .point()
            .ok_or("unnamed point")?
            .clone();
        second.reload()?;
        let stale_unnamed = second.get(unnamed.id()).ok_or("unnamed snapshot")?.clone();
        let temporarily_named = first.rename(&unnamed, Some("temporary"))?;
        let unnamed_again = first.rename(&temporarily_named, None)?;
        assert_eq!(unnamed_again.name(), None);
        assert!(matches!(
            second.rename(&stale_unnamed, None),
            Err(ReviewPointError::StaleRename { .. })
        ));
        Ok(())
    }

    #[test]
    fn historical_duplicate_names_replay_and_allow_fresh_noop()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-historical-names")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let state = dir.0.join("state");
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(&state)?;
        store.capture(&mut workspace, Some("one"))?;
        store.capture(&mut workspace, Some("two"))?;
        drop(store);

        let manifest = state.join(INDEX_FILE);
        let text = fs::read_to_string(&manifest)?;
        fs::write(
            &manifest,
            text.replace(r#""name":"two""#, r#""name":"one""#),
        )?;
        let mut reopened = ReviewPointStore::open(&state)?;
        let duplicates: Vec<_> = reopened
            .list()
            .into_iter()
            .filter(|point| point.name() == Some("one"))
            .collect();
        assert_eq!(duplicates.len(), 2);
        let before = fs::read(&manifest)?;
        reopened.rename(&duplicates[0], Some(" one "))?;
        assert_eq!(fs::read(manifest)?, before);
        Ok(())
    }

    #[test]
    fn historical_invalid_capture_names_remain_readable() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = TempDir::new("review-point-historical-invalid-name")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let state = dir.0.join("state");
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(&state)?;
        let id = store
            .capture(&mut workspace, Some("legacy"))?
            .point()
            .ok_or("legacy point")?
            .id()
            .to_owned();
        drop(store);

        let manifest = state.join(INDEX_FILE);
        let text = fs::read_to_string(&manifest)?;
        let historical = "x".repeat(129);
        fs::write(
            &manifest,
            text.replace(r#""name":"legacy""#, &format!(r#""name":"{historical}""#)),
        )?;
        assert_eq!(
            ReviewPointStore::open(state)?
                .get(&id)
                .and_then(ReviewPoint::name),
            Some(historical.as_str())
        );
        Ok(())
    }

    #[test]
    fn malformed_unknown_and_interrupted_rename_records_fail_closed_or_repair()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-rename-records")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let state = dir.0.join("state");
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(&state)?;
        let point = store
            .capture(&mut workspace, Some("before"))?
            .point()
            .ok_or("point")?
            .clone();
        drop(store);
        let manifest = state.join(INDEX_FILE);
        let original = fs::read(&manifest)?;

        let malformed = format!(
            "{{\"event\":\"{RENAME_EVENT_NAME}\",\"id\":\"{}\",\
             \"previous\":\"before\",\"replacement\":\"after\",\"renamed\":1}}\n",
            point.id()
        );
        let mut bytes = original.clone();
        bytes.extend_from_slice(malformed.as_bytes());
        fs::write(&manifest, bytes)?;
        assert!(matches!(
            ReviewPointStore::open(&state),
            Err(ReviewPointError::Format { .. })
        ));

        let mut bytes = original.clone();
        bytes.extend_from_slice(b"{\"event\":\"review-point-renamed\"}\n");
        fs::write(&manifest, bytes)?;
        assert!(matches!(
            ReviewPointStore::open(&state),
            Err(ReviewPointError::UnexpectedEvent { .. })
        ));

        for (previous, revision) in [("wrong", 1), ("before", 2)] {
            let semantically_invalid = format!(
                "{{\"event\":\"{RENAME_EVENT_NAME}\",\"id\":\"{}\",\
                 \"previous\":\"{previous}\",\"replacement\":\"after\",\
                 \"revision\":{revision},\"renamed\":1}}\n",
                point.id()
            );
            let mut bytes = original.clone();
            bytes.extend_from_slice(semantically_invalid.as_bytes());
            fs::write(&manifest, bytes)?;
            assert!(matches!(
                ReviewPointStore::open(&state),
                Err(ReviewPointError::Invalid(_))
            ));
        }

        let mut interrupted = original.clone();
        interrupted
            .extend_from_slice(format!("{{\"event\":\"{RENAME_EVENT_NAME}\",\"id\":\"").as_bytes());
        fs::write(&manifest, interrupted)?;
        let repaired = ReviewPointStore::open(&state)?;
        assert_eq!(
            repaired.get(point.id()).and_then(ReviewPoint::name),
            Some("before")
        );
        assert_eq!(fs::read(manifest)?, original);
        Ok(())
    }

    #[test]
    fn manifest_append_failure_reports_an_uncertain_capture()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-capture-uncertain")?;
        let state = dir.0.join("state");
        fs::create_dir(&state)?;
        let manifest = state.join(INDEX_FILE);
        fs::write(&manifest, [])?;
        let mut read_only = File::open(manifest)?;
        let point = ReviewPoint {
            id: "uncertain-point".to_owned(),
            name: None,
            name_revision: 0,
            created: 0,
            checkout: dir.0.clone(),
            workspace_key: dir.0.clone(),
            head: None,
            entries: Vec::new(),
            issues: Vec::new(),
        };

        let error = publish_locked(&state, &mut read_only, &point, &[], b"{}\n")
            .err()
            .ok_or("a read-only manifest accepted the append")?;

        assert!(matches!(
            error,
            ReviewPointError::CommitUncertain { point, .. } if point == "uncertain-point"
        ));
        Ok(())
    }

    #[test]
    fn manifest_append_failure_reports_an_uncertain_rename()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-rename-uncertain")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let state = dir.0.join("state");
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(&state)?;
        let point = store
            .capture(&mut workspace, Some("before"))?
            .point()
            .ok_or("point")?
            .clone();
        drop(store);

        let mut read_only = File::open(state.join(INDEX_FILE))?;
        let error = rename_locked(&state, &mut read_only, &point, Some("after"))
            .err()
            .ok_or("a read-only manifest accepted the rename")?;
        assert!(matches!(
            error,
            ReviewPointError::CommitUncertain { point: id, .. } if id == point.id()
        ));
        Ok(())
    }

    #[test]
    fn capture_reports_ignored_paths_without_silently_claiming_their_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-excluded")?;
        init(&dir.0)?;
        fs::write(dir.0.join(".gitignore"), "ignored/\n")?;
        fs::create_dir(dir.0.join("ignored"))?;
        fs::write(dir.0.join("ignored/value.md"), "not captured\n")?;
        let mut workspace = Workspace::discover(&dir.0)?;
        let mut store = ReviewPointStore::open(dir.0.join("state"))?;
        let result = store.capture(&mut workspace, None)?;
        let point = result.point().ok_or("point was not published")?;
        assert!(
            result
                .issues()
                .iter()
                .any(|issue| issue.kind() == CaptureIssueKind::Excluded
                    && issue.path() == Some(Path::new("ignored/value.md")))
        );
        let excluded = store
            .load_bytes(point, &workspace, Path::new("ignored/value.md"))
            .err()
            .ok_or("excluded path was reconstructed")?;
        assert!(excluded.to_string().contains("ignored/value.md"));
        Ok(())
    }

    #[test]
    fn point_compare_uses_saved_bytes_after_worktree_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-compare")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        init(&repo)?;
        commit_and_stage(&repo, &[("tracked.md", "base\n")])?;
        fs::write(repo.join("tracked.md"), "point\n")?;
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(dir.0.join("state"))?;
        let point = store
            .capture(&mut workspace, None)?
            .point()
            .ok_or("point was not published")?
            .clone();
        fs::write(repo.join("tracked.md"), "working\n")?;
        let comparison = store.compare_to_working(&point, &mut workspace)?;
        assert_eq!(comparison.changes().len(), 1);
        assert_eq!(comparison.changes()[0].path(), Path::new("tracked.md"));
        Ok(())
    }

    #[test]
    fn stale_openers_do_not_lose_durable_review_points() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-locking")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        let state = dir.0.join("state");
        let mut workspace_a = Workspace::discover(&repo)?;
        let mut workspace_b = Workspace::discover(&repo)?;
        let mut first_store = ReviewPointStore::open(&state)?;
        let mut second_store = ReviewPointStore::open(&state)?;

        fs::write(repo.join("first.md"), "first\n")?;
        assert!(
            first_store
                .capture(&mut workspace_a, Some("first"))?
                .published()
        );
        fs::write(repo.join("second.md"), "second\n")?;
        assert!(
            second_store
                .capture(&mut workspace_b, Some("second"))?
                .published()
        );

        let reopened = ReviewPointStore::open(state)?;
        assert_eq!(reopened.len(), 2);
        Ok(())
    }

    #[test]
    fn deletion_reclaims_only_unshared_blobs_and_replays() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = TempDir::new("review-point-delete")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("shared.md"), "shared\n")?;
        fs::write(repo.join("changed.md"), "first\n")?;
        let state = dir.0.join("state");
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(&state)?;
        let first = store
            .capture(&mut workspace, Some("first"))?
            .point()
            .ok_or("first point")?
            .clone();
        fs::write(repo.join("changed.md"), "second\n")?;
        let second = store
            .capture(&mut workspace, Some("second"))?
            .point()
            .ok_or("second point")?
            .clone();
        let shared = first
            .entry(Path::new("shared.md"))
            .and_then(ReviewEntry::blob)
            .ok_or("shared blob")?;
        let unique = first
            .entry(Path::new("changed.md"))
            .and_then(ReviewEntry::blob)
            .ok_or("unique blob")?;

        let deleted = store.delete(first.id())?;
        assert_eq!(deleted.point().map(ReviewPoint::id), Some(first.id()));
        assert_eq!(deleted.reclaimed_blobs(), 1);
        assert!(deleted.reclaimed_bytes() > 0);
        assert!(deleted.issues().is_empty());
        assert!(state.join(BLOB_DIR).join(shared).exists());
        assert!(!state.join(BLOB_DIR).join(unique).exists());
        assert!(matches!(
            store.load_bytes(&first, &workspace, Path::new("shared.md")),
            Err(ReviewPointError::Unavailable { .. })
        ));
        assert_eq!(
            store.load_bytes(&second, &workspace, Path::new("shared.md"))?,
            Some(b"shared\n".to_vec())
        );

        let already = store.delete(first.id())?;
        assert!(already.point().is_none());
        drop(store);
        let reopened = ReviewPointStore::open(state)?;
        assert!(reopened.get(first.id()).is_none());
        assert!(reopened.get(second.id()).is_some());
        Ok(())
    }

    #[test]
    fn reload_repairs_only_an_unterminated_final_record() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = TempDir::new("review-point-tail")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let state = dir.0.join("state");
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(&state)?;
        let point = store
            .capture(&mut workspace, None)?
            .point()
            .ok_or("point")?
            .clone();
        drop(store);
        let manifest = state.join(INDEX_FILE);
        let mut file = fs::OpenOptions::new().append(true).open(&manifest)?;
        file.write_all(br#"{"event":"review-point-delete","id":""#)?;
        drop(file);

        let mut reopened = ReviewPointStore::open(&state)?;
        assert!(reopened.get(point.id()).is_some());
        assert_eq!(fs::read(&manifest)?.last(), Some(&b'\n'));
        reopened.delete(point.id())?;
        assert!(ReviewPointStore::open(state)?.get(point.id()).is_none());
        Ok(())
    }

    #[test]
    fn malformed_blob_ids_fail_before_forming_paths() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-bad-blob")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let state = dir.0.join("state");
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(&state)?;
        let point = store
            .capture(&mut workspace, None)?
            .point()
            .ok_or("point")?
            .clone();
        let blob = point.entries()[0].blob().ok_or("blob")?;
        drop(store);
        let manifest = state.join(INDEX_FILE);
        let text = fs::read_to_string(&manifest)?;
        fs::write(&manifest, text.replace(blob, "../outside"))?;

        let error = ReviewPointStore::open(state)
            .err()
            .ok_or("invalid store opened")?;
        assert!(error.to_string().contains("lowercase SHA-256"));
        Ok(())
    }

    #[test]
    fn unsafe_blob_cleanup_is_reported_after_deletion() -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-delete-cleanup")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "point\n")?;
        let state = dir.0.join("state");
        let mut workspace = Workspace::discover(&repo)?;
        let mut store = ReviewPointStore::open(&state)?;
        let point = store
            .capture(&mut workspace, None)?
            .point()
            .ok_or("point")?
            .clone();
        let blob = point.entries()[0].blob().ok_or("blob")?;
        fs::hard_link(state.join(BLOB_DIR).join(blob), state.join("extra-link"))?;

        let deleted = store.delete(point.id())?;
        assert!(deleted.point().is_some());
        assert_eq!(deleted.reclaimed_blobs(), 0);
        assert_eq!(deleted.issues().len(), 1);
        assert_eq!(deleted.issues()[0].blob(), Some(blob));
        assert!(store.get(point.id()).is_none());
        Ok(())
    }

    #[test]
    fn stale_handles_serialize_delete_delete_and_recapture()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = TempDir::new("review-point-delete-stale")?;
        let repo = dir.0.join("repo");
        fs::create_dir(&repo)?;
        fs::write(repo.join("a.md"), "same\n")?;
        let state = dir.0.join("state");
        let mut workspace_a = Workspace::discover(&repo)?;
        let mut workspace_b = Workspace::discover(&repo)?;
        let mut first = ReviewPointStore::open(&state)?;
        let mut second = ReviewPointStore::open(&state)?;
        let point = first
            .capture(&mut workspace_a, Some("first"))?
            .point()
            .ok_or("point")?
            .clone();

        assert!(first.delete(point.id())?.point().is_some());
        assert!(second.delete(point.id())?.point().is_none());
        let replacement = second.capture(&mut workspace_b, Some("replacement"))?;
        let replacement = replacement.point().ok_or("replacement")?;
        assert_eq!(
            second.load_bytes(replacement, &workspace_b, Path::new("a.md"))?,
            Some(b"same\n".to_vec())
        );
        let reopened = ReviewPointStore::open(state)?;
        assert!(reopened.get(point.id()).is_none());
        assert!(reopened.get(replacement.id()).is_some());
        Ok(())
    }
}
