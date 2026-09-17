// @okf-doc: /decisions/0087-global-comparisons-and-board-history.md
//! Explicit workspace review points with content-addressed manifests.
//!
//! A [`ReviewPointStore`] records a deliberate, repository-wide workspace
//! state. Git supplies unchanged committed content; the store keeps changed
//! eligible bytes once per digest and records deletions as tombstones. A
//! point is published only after every required blob and its manifest log
//! record are durable. Capture never writes the checkout, index, refs, or
//! Git object database.
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
use std::fs::{self, File, OpenOptions};
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

/// One deliberate workspace state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPoint {
    id: String,
    name: Option<String>,
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

/// Persistent content-addressed review-point storage.
#[derive(Debug)]
pub struct ReviewPointStore {
    dir: PathBuf,
    points: BTreeMap<String, ReviewPoint>,
    log: File,
}

impl ReviewPointStore {
    /// Open or create a review-point store.
    ///
    /// # Errors
    ///
    /// Returns an I/O or format error when the store cannot be opened.
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, ReviewPointError> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(dir.join(BLOB_DIR))?;
        let log_path = dir.join(INDEX_FILE);
        let mut log = OpenOptions::new()
            .append(true)
            .read(true)
            .create(true)
            .open(&log_path)?;
        let mut lock = FileLock::shared(&mut log)?;
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
    #[expect(
        clippy::too_many_lines,
        reason = "capture keeps the pre-publication manifest validation in one transaction"
    )]
    pub fn capture(
        &mut self,
        workspace: &mut Workspace,
        name: Option<&str>,
    ) -> Result<CaptureResult, ReviewPointError> {
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
        let id = point_id(created, workspace, name, head.as_ref(), &entries);
        let point = ReviewPoint {
            id,
            name: name.map(str::to_owned),
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
        if let Some(entry) = point.entry(path) {
            return match &entry.content {
                ReviewContent::Tombstone => Ok(None),
                ReviewContent::Blob(blob) => self.read_blob(blob, point, path).map(Some),
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
        let baseline = point
            .head()
            .cloned()
            .map_or(ComparisonEndpoint::EmptyTree, ComparisonEndpoint::Commit);
        let mut point_files = workspace.endpoint_files(&baseline)?;
        for entry in &point.entries {
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
        let mut paths = BTreeSet::new();
        paths.extend(point_files.keys().cloned());
        paths.extend(working_files.keys().cloned());
        let mut changes = Vec::new();
        for path in paths {
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
            changes,
        ))
    }

    fn read_blob(
        &self,
        blob: &str,
        point: &ReviewPoint,
        path: &Path,
    ) -> Result<Vec<u8>, ReviewPointError> {
        fs::read(self.dir.join(BLOB_DIR).join(blob)).map_err(|error| {
            ReviewPointError::MissingBacking {
                point: point.id.clone(),
                path: path.to_path_buf(),
                detail: format!("content blob {blob} is unavailable: {error}"),
            }
        })
    }

    fn publish(
        &mut self,
        point: &ReviewPoint,
        blobs: &[(String, Vec<u8>)],
    ) -> Result<(), ReviewPointError> {
        let event = StoredEvent::from_point(point);
        let mut line = serde_json::to_vec(&event).map_err(|source| ReviewPointError::Json {
            detail: source.to_string(),
        })?;
        line.push(b'\n');
        let mut lock = FileLock::exclusive(&mut self.log)?;
        let result = publish_locked(&self.dir, lock.file_mut(), point, blobs, &line);
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ReviewContent {
    Blob(String),
    Tombstone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredEvent {
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

impl StoredEvent {
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
    fn shared(file: &'a mut File) -> io::Result<Self> {
        file.lock_shared()?;
        Ok(Self { file, locked: true })
    }

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
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let mut points = BTreeMap::new();
    for (line_number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: StoredEvent =
            serde_json::from_str(line).map_err(|source| ReviewPointError::Format {
                line: line_number + 1,
                detail: source.to_string(),
            })?;
        if event.kind != EVENT_NAME {
            return Err(ReviewPointError::UnexpectedEvent {
                line: line_number + 1,
                event: event.kind,
            });
        }
        let point = event.into_point()?;
        points.insert(point.id.clone(), point);
    }
    Ok(points)
}

fn publish_locked(
    dir: &Path,
    file: &mut File,
    point: &ReviewPoint,
    blobs: &[(String, Vec<u8>)],
    line: &[u8],
) -> Result<BTreeMap<String, ReviewPoint>, ReviewPointError> {
    for (blob, bytes) in blobs {
        ensure_blob(dir, blob, bytes)?;
    }
    let mut points = replay_log(file)?;
    file.seek(SeekFrom::End(0))?;
    file.write_all(line)?;
    file.flush()?;
    file.sync_all()?;
    sync_directory(dir)?;
    points.insert(point.id.clone(), point.clone());
    Ok(points)
}

fn ensure_blob(dir: &Path, blob: &str, bytes: &[u8]) -> io::Result<()> {
    let directory = dir.join(BLOB_DIR);
    let target = directory.join(blob);
    if target.is_file() {
        return Ok(());
    }
    let temporary = directory.join(format!(".{blob}.{}.tmp", nonce()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    fs::rename(&temporary, &target)?;
    sync_directory(&directory)?;
    Ok(())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
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
}
