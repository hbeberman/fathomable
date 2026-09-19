// @okf-doc: /decisions/0028-live-workspace.md
//! Workspace watcher: turns `notify` events into tree, view, and thread
//! changes (ADR 0028).
//!
//! [`Watcher`] keeps narrow thread and Git watches on the event-loop thread.
//! One cancellable standard thread discovers and installs broad workspace
//! watches up to [`WatchLimits`], prioritizing the root, loaded documents,
//! and materialized tree directories. Ignored build trees consume neither
//! inotify watches nor event-loop work, and symlink directories are not
//! traversed.
//! Raw events go into a [`Batch`], which waits out the hint debounce and
//! then folds a burst into one [`Event`] per path: a plain
//! [`Event::Change`], a [`Event::Created`] or [`Event::Removed`] entry, or
//! a [`Event::Renamed`] pair. [`Event::Rescan`] preserves the platform's
//! warning that events were lost. Renames are paired from the platform's
//! own pairing first; an unpaired remove-then-create in one batch is
//! paired when the created file's size and content hash match what the
//! removed path last held.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{self, Read};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc as std_mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use fathomable_core::annotations::short_hash;
use fathomable_core::follow::Ignore;
use fathomable_core::workspace::{EntryKind, Workspace, is_rules_file};
use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;

use super::App;

/// Finite resource limits for broad workspace observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WatchLimits {
    workspace_watches: NonZeroUsize,
    discovery_entries: NonZeroUsize,
    retained_paths: NonZeroUsize,
    pending_events: NonZeroUsize,
}

impl WatchLimits {
    /// Build limits when every dimension is positive.
    #[must_use]
    pub(crate) fn new(
        workspace_watches: usize,
        discovery_entries: usize,
        retained_paths: usize,
        pending_events: usize,
    ) -> Option<Self> {
        Some(Self {
            workspace_watches: NonZeroUsize::new(workspace_watches)?,
            discovery_entries: NonZeroUsize::new(discovery_entries)?,
            retained_paths: NonZeroUsize::new(retained_paths)?,
            pending_events: NonZeroUsize::new(pending_events)?,
        })
    }

    #[must_use]
    pub(crate) const fn workspace_watches(self) -> usize {
        self.workspace_watches.get()
    }

    #[must_use]
    pub(crate) const fn discovery_entries(self) -> usize {
        self.discovery_entries.get()
    }

    #[must_use]
    pub(crate) const fn retained_paths(self) -> usize {
        self.retained_paths.get()
    }

    #[must_use]
    pub(crate) const fn pending_events(self) -> usize {
        self.pending_events.get()
    }
}

impl Default for WatchLimits {
    fn default() -> Self {
        // These defaults cap broad inotify use well below common per-user
        // limits while allowing an ordinary monorepo to finish. Discovery
        // and queues have separate ceilings so a single wide directory
        // cannot allocate in proportion to an arbitrary workspace.
        Self::new(8_192, 100_000, 50_000, 4_096).unwrap_or(Self {
            workspace_watches: NonZeroUsize::MIN,
            discovery_entries: NonZeroUsize::MIN,
            retained_paths: NonZeroUsize::MIN,
            pending_events: NonZeroUsize::MIN,
        })
    }
}

/// Why broad workspace coverage stopped before reaching the whole tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LimitReason {
    WorkspaceWatches,
    DiscoveryEntries,
    RetainedPaths,
    ControlWatches,
}

impl std::fmt::Display for LimitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::WorkspaceWatches => "workspace watch limit reached",
            Self::DiscoveryEntries => "directory-entry examination limit reached",
            Self::RetainedPaths => "pending-path retention limit reached",
            Self::ControlWatches => "Git control watch limit reached",
        })
    }
}

/// Persistent broad workspace observation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WatchStatus {
    Scanning {
        generation: u64,
        watched: usize,
        examined: usize,
    },
    Complete {
        watched: usize,
        examined: usize,
    },
    Limited {
        watched: usize,
        examined: usize,
        reason: LimitReason,
    },
    Errored {
        watched: usize,
        examined: usize,
        reason: String,
    },
}

impl Default for WatchStatus {
    fn default() -> Self {
        Self::Scanning {
            generation: 0,
            watched: 0,
            examined: 0,
        }
    }
}

impl WatchStatus {
    /// Stable text for the status overlay.
    #[must_use]
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Scanning {
                watched, examined, ..
            } => format!("scanning ({watched} watched, {examined} entries examined)"),
            Self::Complete { watched, examined } => {
                format!("complete ({watched} watched, {examined} entries examined)")
            }
            Self::Limited {
                watched,
                examined,
                reason,
            } => format!("limited: {reason} ({watched} watched, {examined} entries examined)"),
            Self::Errored {
                watched,
                examined,
                reason,
            } => format!("errored: {reason} ({watched} watched, {examined} entries examined)"),
        }
    }
}

/// What a settled batch says happened to one path. Paths are absolute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    /// The platform lost events; reconcile from the filesystem.
    Rescan,
    /// The file's content may have changed.
    Change(PathBuf),
    /// A file or directory appeared.
    Created(PathBuf),
    /// A file or directory vanished.
    Removed(PathBuf),
    /// A file or directory moved from `from` to `to`.
    Renamed { from: PathBuf, to: PathBuf },
}

impl Event {
    /// The path the event lands on: the new name for a rename, or no path
    /// when the whole workspace needs a rescan.
    #[must_use]
    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Self::Rescan => None,
            Self::Change(path) | Self::Created(path) | Self::Removed(path) => Some(path),
            Self::Renamed { to, .. } => Some(to),
        }
    }
}

/// The size and content hash of a file, for pairing an unpaired
/// remove-then-create as a rename.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Fingerprint {
    size: u64,
    hash: String,
}

impl Fingerprint {
    /// The fingerprint of `bytes`.
    #[must_use]
    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            hash: short_hash(bytes),
        }
    }

    /// The fingerprint of the file at `path`, when it can be read and
    /// holds at most `max_bytes`: a larger one is read by nobody, so it
    /// pairs with nothing (`viewer.max-file-size-mib`).
    fn read(path: &Path, max_bytes: u64) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        if !meta.is_file() || meta.len() > max_bytes {
            return None;
        }
        let mut bytes = Vec::new();
        fs::File::open(path)
            .ok()?
            .take(max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .ok()?;
        (u64::try_from(bytes.len()).ok()? <= max_bytes).then(|| Self::from_bytes(&bytes))
    }
}

/// One raw watcher notification, before debouncing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Raw {
    /// The platform lost events; all remembered state may be stale.
    Rescan,
    /// A file or directory appeared.
    Create(PathBuf),
    /// A file's content or metadata changed.
    Modify(PathBuf),
    /// A file or directory vanished.
    Remove(PathBuf),
    /// The old name of a rename whose new name may follow.
    RenameFrom(PathBuf),
    /// The new name of a rename whose old name may have come before.
    RenameTo(PathBuf),
    /// A rename the platform paired itself.
    Rename { from: PathBuf, to: PathBuf },
}

impl Raw {
    /// Translate a `notify` event; `None` for events that change neither
    /// content nor existence (opens and other accesses).
    fn from_notify(event: notify::Event) -> Vec<Self> {
        if event.need_rescan() {
            return vec![Self::Rescan];
        }
        let notify::Event { kind, paths, .. } = event;
        match kind {
            EventKind::Create(_) => paths.into_iter().map(Self::Create).collect(),
            EventKind::Remove(_) => paths.into_iter().map(Self::Remove).collect(),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                paths.into_iter().map(Self::RenameFrom).collect()
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                paths.into_iter().map(Self::RenameTo).collect()
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
                let mut paths = paths.into_iter();
                match (paths.next(), paths.next()) {
                    (Some(from), Some(to)) => vec![Self::Rename { from, to }],
                    (Some(only), None) => vec![Self::Modify(only)],
                    _ => Vec::new(),
                }
            }
            EventKind::Modify(_) => paths.into_iter().map(Self::Modify).collect(),
            EventKind::Access(_) | EventKind::Any | EventKind::Other => Vec::new(),
        }
    }

    /// Every path named by this raw event.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        let (first, second) = match self {
            Self::Rescan => (None, None),
            Self::Create(path)
            | Self::Modify(path)
            | Self::Remove(path)
            | Self::RenameFrom(path)
            | Self::RenameTo(path) => (Some(path.as_path()), None),
            Self::Rename { from, to } => (Some(from.as_path()), Some(to.as_path())),
        };
        [first, second].into_iter().flatten()
    }
}

/// Raw events from the platform watcher, in arrival order.
#[derive(Debug)]
pub(crate) struct Raws {
    rx: mpsc::Receiver<Raw>,
    overflowed: Arc<AtomicU64>,
}

impl Raws {
    /// The next event; `None` once the watcher is gone.
    pub(crate) async fn recv(&mut self) -> Option<Raw> {
        if self.overflowed.swap(0, Ordering::AcqRel) != 0 {
            return Some(Raw::Rescan);
        }
        let raw = self.rx.recv().await;
        if self.overflowed.swap(0, Ordering::AcqRel) != 0 {
            Some(Raw::Rescan)
        } else {
            raw
        }
    }

    /// An event already waiting, without blocking.
    pub(crate) fn try_recv(&mut self) -> Option<Raw> {
        if self.overflowed.swap(0, Ordering::AcqRel) != 0 {
            return Some(Raw::Rescan);
        }
        self.rx.try_recv().ok()
    }

    #[cfg(test)]
    pub(crate) fn test_channel() -> (mpsc::Sender<Raw>, Self) {
        let (tx, rx) = mpsc::channel(4_096);
        (
            tx,
            Self {
                rx,
                overflowed: Arc::new(AtomicU64::new(0)),
            },
        )
    }
}

#[derive(Clone)]
struct RawSender {
    tx: mpsc::Sender<Raw>,
    overflowed: Arc<AtomicU64>,
}

impl RawSender {
    fn send(&self, raw: Raw) {
        match self.tx.try_send(raw) {
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.overflowed.store(1, Ordering::Release);
            }
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}

fn notify_watcher(sender: RawSender) -> notify::Result<notify::RecommendedWatcher> {
    notify::recommended_watcher(move |result: notify::Result<notify::Event>| match result {
        Ok(event) => {
            tracing::debug!(kind = ?event.kind, paths = ?event.paths, "watcher event");
            for raw in Raw::from_notify(event) {
                sender.send(raw);
            }
        }
        Err(error) => {
            tracing::warn!(%error, "file watcher error");
            sender.send(Raw::Rescan);
        }
    })
}

#[derive(Debug, Clone, Copy)]
enum Surface {
    State,
    Extras,
}

impl Surface {
    const fn bit(self) -> u8 {
        match self {
            Self::State => 1 << 0,
            Self::Extras => 1 << 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Coverage(u8);

impl Coverage {
    fn set(&mut self, surface: Surface, complete: bool) {
        if complete {
            self.0 &= !surface.bit();
        } else {
            self.0 |= surface.bit();
        }
    }

    const fn complete(self) -> bool {
        self.0 == 0
    }
}

const DISCOVERY_CHUNK: usize = 128;
const DISCOVERY_RESULTS: usize = 8;
// State and immediate Git metadata share this small UI-owned allowance.
// Broad workspace and recursive refs coverage use the configured worker budget.
const CONTROL_WATCH_LIMIT: usize = 16;

#[derive(Debug, Clone)]
struct DiscoveryRequest {
    generation: u64,
    root: PathBuf,
    priorities: Vec<PathBuf>,
    git_roots: Vec<PathBuf>,
    git_roots_limited: bool,
    git_priorities: Vec<PathBuf>,
    synthetic_roots: Vec<PathBuf>,
    ignore: Ignore,
    limits: WatchLimits,
    #[cfg(test)]
    fail_on: Option<PathBuf>,
}

#[derive(Debug)]
pub(crate) enum DiscoveryOutcome {
    Complete,
    Limited(LimitReason),
    Errored(String),
    Cancelled,
}

#[derive(Debug)]
pub(crate) enum DiscoveryUpdate {
    Progress {
        generation: u64,
        paths: Vec<PathBuf>,
        examined: usize,
    },
    Finished {
        generation: u64,
        watched: usize,
        examined: usize,
        outcome: DiscoveryOutcome,
    },
}

/// Results from the dedicated broad-watch worker.
#[derive(Debug)]
pub(crate) struct Discoveries {
    rx: mpsc::Receiver<DiscoveryUpdate>,
}

impl Discoveries {
    pub(crate) async fn recv(&mut self) -> Option<DiscoveryUpdate> {
        self.rx.recv().await
    }
}

#[derive(Debug)]
struct DiscoveryControl {
    tx: std_mpsc::SyncSender<DiscoveryRequest>,
    generation: Arc<AtomicU64>,
    pending: Option<DiscoveryRequest>,
    root: Option<PathBuf>,
    priorities: Vec<PathBuf>,
    git_roots: Vec<PathBuf>,
    git_roots_limited: bool,
    git_priorities: Vec<PathBuf>,
    ignore: Ignore,
    limits: WatchLimits,
}

impl DiscoveryControl {
    #[expect(
        clippy::too_many_arguments,
        reason = "one generation snapshot carries each bounded discovery surface"
    )]
    fn request(
        &mut self,
        root: PathBuf,
        priorities: Vec<PathBuf>,
        git_roots: Vec<PathBuf>,
        git_roots_limited: bool,
        git_priorities: Vec<PathBuf>,
        synthetic_roots: Vec<PathBuf>,
        ignore: Ignore,
    ) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.root = Some(root.clone());
        self.priorities.clone_from(&priorities);
        self.git_roots.clone_from(&git_roots);
        self.git_roots_limited = git_roots_limited;
        self.git_priorities.clone_from(&git_priorities);
        self.ignore.clone_from(&ignore);
        let request = DiscoveryRequest {
            generation,
            root,
            priorities,
            git_roots,
            git_roots_limited,
            git_priorities,
            synthetic_roots,
            ignore,
            limits: self.limits,
            #[cfg(test)]
            fail_on: None,
        };
        match self.tx.try_send(request) {
            Err(std_mpsc::TrySendError::Full(request)) => self.pending = Some(request),
            Ok(()) | Err(std_mpsc::TrySendError::Disconnected(_)) => self.pending = None,
        }
        generation
    }

    fn flush_pending(&mut self) {
        let Some(request) = self.pending.take() else {
            return;
        };
        match self.tx.try_send(request) {
            Err(std_mpsc::TrySendError::Full(request)) => self.pending = Some(request),
            Ok(()) | Err(std_mpsc::TrySendError::Disconnected(_)) => {}
        }
    }
}

impl Drop for DiscoveryControl {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

fn spawn_discovery(
    mut watcher: notify::RecommendedWatcher,
    raw: RawSender,
    limits: WatchLimits,
) -> io::Result<(DiscoveryControl, Discoveries)> {
    let (commands_tx, commands_rx) = std_mpsc::sync_channel::<DiscoveryRequest>(1);
    let (results_tx, results_rx) = mpsc::channel::<DiscoveryUpdate>(DISCOVERY_RESULTS);
    let generation = Arc::new(AtomicU64::new(0));
    let worker_generation = Arc::clone(&generation);
    thread::Builder::new()
        .name("workspace-watch".to_owned())
        .spawn(move || {
            let mut installed: HashSet<PathBuf> = HashSet::new();
            while let Ok(mut request) = commands_rx.recv() {
                while let Ok(newer) = commands_rx.try_recv() {
                    request = newer;
                }
                for path in installed.drain() {
                    let _ = watcher.unwatch(&path);
                }
                if worker_generation.load(Ordering::Acquire) != request.generation {
                    send_finished(&results_tx, &request, 0, 0, DiscoveryOutcome::Cancelled);
                    continue;
                }
                run_discovery(
                    &mut watcher,
                    &mut installed,
                    &request,
                    &worker_generation,
                    &results_tx,
                    &raw,
                );
            }
        })?;
    Ok((
        DiscoveryControl {
            tx: commands_tx,
            generation,
            pending: None,
            root: None,
            priorities: Vec::new(),
            git_roots: Vec::new(),
            git_roots_limited: false,
            git_priorities: Vec::new(),
            ignore: Ignore::default(),
            limits,
        },
        Discoveries { rx: results_rx },
    ))
}

#[derive(Debug)]
enum PendingDir {
    Workspace(PathBuf),
    Git(PathBuf),
}

impl PendingDir {
    fn path(&self) -> &Path {
        match self {
            Self::Workspace(path) | Self::Git(path) => path,
        }
    }
}

fn retain_pending(
    pending: &mut VecDeque<PendingDir>,
    retained: &mut HashSet<PathBuf>,
    dir: PendingDir,
    limit: usize,
) -> bool {
    if retained.contains(dir.path()) {
        return true;
    }
    if retained.len() >= limit {
        return false;
    }
    retained.insert(dir.path().to_path_buf());
    pending.push_back(dir);
    true
}

fn retain_pending_front(
    pending: &mut VecDeque<PendingDir>,
    retained: &mut HashSet<PathBuf>,
    dir: PendingDir,
    limit: usize,
) -> bool {
    if retained.contains(dir.path()) {
        return true;
    }
    if retained.len() >= limit {
        return false;
    }
    retained.insert(dir.path().to_path_buf());
    pending.push_front(dir);
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallResult {
    Ready,
    Skipped,
    Full,
    Abort,
}

#[expect(
    clippy::too_many_arguments,
    reason = "watch installation reports into one generation's bounded worker state"
)]
fn install_directory(
    watcher: &mut notify::RecommendedWatcher,
    installed: &mut HashSet<PathBuf>,
    path: &Path,
    request: &DiscoveryRequest,
    current_generation: &AtomicU64,
    results: &mpsc::Sender<DiscoveryUpdate>,
    progress: &mut Vec<PathBuf>,
    examined: usize,
    first_error: &mut Option<String>,
) -> InstallResult {
    if current_generation.load(Ordering::Acquire) != request.generation {
        send_finished(
            results,
            request,
            installed.len(),
            examined,
            DiscoveryOutcome::Cancelled,
        );
        return InstallResult::Abort;
    }
    if installed.contains(path) {
        return InstallResult::Ready;
    }
    #[cfg(test)]
    if request.fail_on.as_deref() == Some(path) {
        first_error.get_or_insert_with(|| {
            format!(
                "cannot read {}: permission denied (injected)",
                path.display()
            )
        });
        return InstallResult::Skipped;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return InstallResult::Skipped,
        Err(error) => {
            first_error.get_or_insert_with(|| format!("cannot inspect directory: {error}"));
            return InstallResult::Skipped;
        }
    }
    if installed.len() >= request.limits.workspace_watches() {
        return InstallResult::Full;
    }
    if let Err(error) = watcher.watch(path, RecursiveMode::NonRecursive) {
        first_error.get_or_insert_with(|| format!("cannot install workspace watch: {error}"));
        return InstallResult::Skipped;
    }
    installed.insert(path.to_path_buf());
    progress.push(path.to_path_buf());
    if progress.len() == DISCOVERY_CHUNK
        && !send_progress(
            results,
            request.generation,
            progress,
            examined,
            current_generation,
        )
    {
        return InstallResult::Abort;
    }
    InstallResult::Ready
}

#[expect(
    clippy::too_many_lines,
    reason = "the bounded walk keeps cancellation, accounting, and each limit in one loop"
)]
fn run_discovery(
    watcher: &mut notify::RecommendedWatcher,
    installed: &mut HashSet<PathBuf>,
    request: &DiscoveryRequest,
    current_generation: &AtomicU64,
    results: &mpsc::Sender<DiscoveryUpdate>,
    raw: &RawSender,
) {
    let mut workspace = match Workspace::discover(&request.root) {
        Ok(workspace) => workspace,
        Err(error) => {
            send_finished(
                results,
                request,
                0,
                0,
                DiscoveryOutcome::Errored(error.to_string()),
            );
            return;
        }
    };
    let mut limited = request
        .git_roots_limited
        .then_some(LimitReason::RetainedPaths);
    let mut priority_paths = VecDeque::new();
    let mut priority_retained = HashSet::new();
    let retain_priority =
        |paths: &mut VecDeque<PathBuf>, retained: &mut HashSet<PathBuf>, path: PathBuf| {
            if retained.contains(&path) {
                return true;
            }
            if retained.len() >= request.limits.retained_paths() {
                return false;
            }
            retained.insert(path.clone());
            paths.push_back(path);
            true
        };
    if !retain_priority(
        &mut priority_paths,
        &mut priority_retained,
        request.root.clone(),
    ) {
        limited = Some(LimitReason::RetainedPaths);
    }
    for path in request.git_priorities.iter().chain(&request.git_roots) {
        if !retain_priority(&mut priority_paths, &mut priority_retained, path.clone()) {
            limited = Some(LimitReason::RetainedPaths);
            break;
        }
    }
    'priorities: for priority in &request.priorities {
        let Ok(relative) = priority.strip_prefix(&request.root) else {
            if !retain_priority(
                &mut priority_paths,
                &mut priority_retained,
                priority.clone(),
            ) {
                limited = Some(LimitReason::RetainedPaths);
                break;
            }
            continue;
        };
        let mut absolute = request.root.clone();
        if !retain_priority(
            &mut priority_paths,
            &mut priority_retained,
            absolute.clone(),
        ) {
            limited = Some(LimitReason::RetainedPaths);
            break;
        }
        for component in relative.components() {
            absolute.push(component.as_os_str());
            if !retain_priority(
                &mut priority_paths,
                &mut priority_retained,
                absolute.clone(),
            ) {
                limited = Some(LimitReason::RetainedPaths);
                break 'priorities;
            }
        }
    }
    let mut examined = 0_usize;
    let mut first_error = None;
    let mut progress = Vec::with_capacity(DISCOVERY_CHUNK);
    while let Some(path) = priority_paths.pop_front() {
        match install_directory(
            watcher,
            installed,
            &path,
            request,
            current_generation,
            results,
            &mut progress,
            examined,
            &mut first_error,
        ) {
            InstallResult::Ready | InstallResult::Skipped => {}
            InstallResult::Full => {
                limited = Some(LimitReason::WorkspaceWatches);
                break;
            }
            InstallResult::Abort => return,
        }
    }

    let mut pending = VecDeque::new();
    let mut retained = HashSet::new();
    if !retain_pending(
        &mut pending,
        &mut retained,
        PendingDir::Workspace(request.root.clone()),
        request.limits.retained_paths(),
    ) {
        limited = Some(LimitReason::RetainedPaths);
    }
    for root in &request.git_roots {
        if !retain_pending(
            &mut pending,
            &mut retained,
            PendingDir::Git(root.clone()),
            request.limits.retained_paths(),
        ) {
            limited = Some(LimitReason::RetainedPaths);
            break;
        }
    }
    for priority in request.git_priorities.iter().rev() {
        if !retain_pending_front(
            &mut pending,
            &mut retained,
            PendingDir::Git(priority.clone()),
            request.limits.retained_paths(),
        ) {
            limited = Some(LimitReason::RetainedPaths);
            break;
        }
    }
    let mut stop_walk = installed.len() >= request.limits.workspace_watches();
    if stop_walk && !pending.is_empty() {
        limited = Some(LimitReason::WorkspaceWatches);
    }
    while let Some(dir) = pending.pop_front() {
        if stop_walk {
            break;
        }
        let workspace_dir = matches!(dir, PendingDir::Workspace(_));
        let absolute = dir.path();
        match install_directory(
            watcher,
            installed,
            absolute,
            request,
            current_generation,
            results,
            &mut progress,
            examined,
            &mut first_error,
        ) {
            InstallResult::Ready => {}
            InstallResult::Skipped => continue,
            InstallResult::Full => {
                limited = Some(LimitReason::WorkspaceWatches);
                break;
            }
            InstallResult::Abort => return,
        }

        let relative = workspace_dir
            .then(|| absolute.strip_prefix(&request.root).ok())
            .flatten();
        let entries = match fs::read_dir(absolute) {
            Ok(entries) => entries,
            Err(error) => {
                first_error.get_or_insert_with(|| {
                    format!("cannot read directory {}: {error}", absolute.display())
                });
                continue;
            }
        };
        for item in entries {
            if current_generation.load(Ordering::Acquire) != request.generation {
                send_finished(
                    results,
                    request,
                    installed.len(),
                    examined,
                    DiscoveryOutcome::Cancelled,
                );
                return;
            }
            if examined >= request.limits.discovery_entries() {
                limited = Some(LimitReason::DiscoveryEntries);
                stop_walk = true;
                break;
            }
            examined += 1;
            let item = match item {
                Ok(item) => item,
                Err(error) => {
                    first_error
                        .get_or_insert_with(|| format!("cannot enumerate directory: {error}"));
                    continue;
                }
            };
            let name = item.file_name();
            if relative.is_some() && name == ".git" {
                continue;
            }
            let child_relative = relative.map(|path| path.join(&name));
            let child_absolute = item.path();
            let file_type = match item.file_type() {
                Ok(file_type) => file_type,
                Err(error) => {
                    first_error
                        .get_or_insert_with(|| format!("cannot inspect directory entry: {error}"));
                    continue;
                }
            };
            if file_type.is_dir() && !file_type.is_symlink() {
                if child_relative.as_ref().is_some_and(|child| {
                    request.ignore.is_tree_ignored(child)
                        || workspace.is_ignored(child, EntryKind::Dir)
                }) {
                    continue;
                }
                let child = if workspace_dir {
                    PendingDir::Workspace(child_absolute)
                } else {
                    PendingDir::Git(child_absolute)
                };
                let retained_child = if workspace_dir {
                    retain_pending(
                        &mut pending,
                        &mut retained,
                        child,
                        request.limits.retained_paths(),
                    )
                } else {
                    retain_pending_front(
                        &mut pending,
                        &mut retained,
                        child,
                        request.limits.retained_paths(),
                    )
                };
                if !retained_child {
                    limited = Some(LimitReason::RetainedPaths);
                    stop_walk = true;
                    break;
                }
            } else if child_relative.as_ref().is_some_and(|child_relative| {
                request
                    .synthetic_roots
                    .iter()
                    .any(|root| child_absolute.starts_with(root))
                    && !request.ignore.is_ignored(child_relative)
                    && !workspace.is_ignored(child_relative, EntryKind::File)
            }) {
                raw.send(Raw::Create(child_absolute));
            }
        }
        if stop_walk {
            break;
        }
    }
    if !progress.is_empty()
        && !send_progress(
            results,
            request.generation,
            &mut progress,
            examined,
            current_generation,
        )
    {
        return;
    }
    let outcome = if let Some(reason) = limited {
        DiscoveryOutcome::Limited(reason)
    } else if let Some(reason) = first_error {
        DiscoveryOutcome::Errored(reason)
    } else {
        DiscoveryOutcome::Complete
    };
    send_finished(results, request, installed.len(), examined, outcome);
}

fn send_progress(
    results: &mpsc::Sender<DiscoveryUpdate>,
    generation: u64,
    progress: &mut Vec<PathBuf>,
    examined: usize,
    current_generation: &AtomicU64,
) -> bool {
    if current_generation.load(Ordering::Acquire) != generation {
        return false;
    }
    let paths = std::mem::replace(progress, Vec::with_capacity(DISCOVERY_CHUNK));
    results
        .blocking_send(DiscoveryUpdate::Progress {
            generation,
            paths,
            examined,
        })
        .is_ok()
}

fn send_finished(
    results: &mpsc::Sender<DiscoveryUpdate>,
    request: &DiscoveryRequest,
    watched: usize,
    examined: usize,
    outcome: DiscoveryOutcome,
) {
    let _ = results.blocking_send(DiscoveryUpdate::Finished {
        generation: request.generation,
        watched,
        examined,
        outcome,
    });
}

/// Watches visible directories and the small explicit state surfaces.
///
/// The workspace tree is covered by non-recursive watches so Git-ignored
/// build trees never spend the process-wide inotify budget. Loaded files'
/// ancestor directories are covered separately, so files opened from an
/// ignored directory still reload and follow renames in the background.
pub(crate) struct Watcher {
    inner: notify::RecommendedWatcher,
    /// Every loaded document, including background documents.
    targets: HashSet<PathBuf>,
    /// Visible directories under the active root.
    root_dirs: HashSet<PathBuf>,
    /// Root and demand-driven directories accepted before worker progress lands.
    priority_dirs: HashSet<PathBuf>,
    /// Newly created subtrees whose synthetic population events are admitted.
    synthetic_roots: Vec<PathBuf>,
    /// Newly-created directories under registered Git refs, newest first.
    git_priorities: Vec<PathBuf>,
    /// Directories holding exact state files.
    state_dirs: HashSet<PathBuf>,
    /// Parents of state directories, watched for remove/recreate/replacement.
    state_parent_dirs: HashSet<PathBuf>,
    /// The exact thread-state paths.
    state_files: HashSet<PathBuf>,
    /// Immediate, non-recursive Git metadata paths.
    extras: HashSet<PathBuf>,
    /// The watches currently installed in `notify`.
    watched: HashSet<PathBuf>,
    /// The active worktree root.
    root: Option<PathBuf>,
    limits: WatchLimits,
    discovery: DiscoveryControl,
    discoveries: Option<Discoveries>,
    generation: u64,
    status: WatchStatus,
    coverage: Coverage,
    control_limited: bool,
}

impl std::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watcher")
            .field("targets", &self.targets)
            .field("visible_directories", &self.root_dirs.len())
            .field("state_files", &self.state_files)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Watcher {
    /// Start a watcher; raw events arrive on the returned [`Raws`].
    ///
    /// # Errors
    ///
    /// Fails when the platform watcher cannot be created.
    #[cfg(test)]
    pub(crate) fn new() -> anyhow::Result<(Self, Raws)> {
        Self::with_limits(WatchLimits::default())
    }

    /// Start a watcher with explicit finite broad-coverage limits.
    pub(crate) fn with_limits(limits: WatchLimits) -> anyhow::Result<(Self, Raws)> {
        let (tx, rx) = mpsc::channel::<Raw>(limits.pending_events());
        let overflowed = Arc::new(AtomicU64::new(0));
        let sender = RawSender {
            tx,
            overflowed: Arc::clone(&overflowed),
        };
        let watcher = notify_watcher(sender.clone()).context("cannot create file watcher")?;
        let broad = notify_watcher(sender.clone()).context("cannot create workspace watcher")?;
        let (discovery, discoveries) = spawn_discovery(broad, sender, limits)
            .context("cannot start workspace watch worker")?;
        Ok((
            Self {
                inner: watcher,
                targets: HashSet::new(),
                root_dirs: HashSet::new(),
                priority_dirs: HashSet::new(),
                synthetic_roots: Vec::new(),
                git_priorities: Vec::new(),
                state_dirs: HashSet::new(),
                state_parent_dirs: HashSet::new(),
                state_files: HashSet::new(),
                extras: HashSet::new(),
                watched: HashSet::new(),
                root: None,
                limits,
                discovery,
                discoveries: Some(discoveries),
                generation: 0,
                status: WatchStatus::default(),
                coverage: Coverage::default(),
                control_limited: false,
            },
            Raws { rx, overflowed },
        ))
    }

    /// Hand the discovery result receiver to the event loop.
    pub(crate) fn take_discoveries(&mut self) -> Option<Discoveries> {
        self.discoveries.take()
    }

    /// Start or reprioritize broad discovery without traversing on this thread.
    pub(crate) fn discover(
        &mut self,
        root: PathBuf,
        priorities: Vec<PathBuf>,
        ignore: Ignore,
        force: bool,
    ) -> bool {
        if !force
            && self.discovery.root.as_ref() == Some(&root)
            && self.discovery.priorities == priorities
        {
            return false;
        }
        self.root = Some(root.clone());
        self.root_dirs.clear();
        self.priority_dirs = priorities.iter().cloned().collect();
        self.priority_dirs.insert(root.clone());
        self.synthetic_roots.clear();
        let git_roots = self.discovery.git_roots.clone();
        let git_roots_limited = self.discovery.git_roots_limited;
        self.generation = self.discovery.request(
            root,
            priorities,
            git_roots,
            git_roots_limited,
            self.git_priorities.clone(),
            Vec::new(),
            ignore,
        );
        self.status = WatchStatus::Scanning {
            generation: self.generation,
            watched: 0,
            examined: 0,
        };
        true
    }

    /// Remember exact loaded files for event admission.
    pub(crate) fn follow<'a>(&mut self, targets: impl IntoIterator<Item = &'a Path>) {
        let targets = targets.into_iter().map(Path::to_path_buf).collect();
        self.targets = targets;
    }

    /// Restart discovery with a created subtree first and synthetic file events.
    pub(crate) fn discover_created(
        &mut self,
        priorities: Vec<PathBuf>,
        ignore: Ignore,
        created: PathBuf,
    ) -> bool {
        let Some(root) = self.root.clone() else {
            return false;
        };
        self.root_dirs.clear();
        self.priority_dirs = priorities.iter().cloned().collect();
        self.priority_dirs.insert(root.clone());
        self.priority_dirs.insert(created.clone());
        if !self.synthetic_roots.contains(&created)
            && self.synthetic_roots.len() < self.limits.retained_paths()
        {
            self.synthetic_roots.push(created);
        }
        let git_roots = self.discovery.git_roots.clone();
        let git_roots_limited = self.discovery.git_roots_limited;
        self.generation = self.discovery.request(
            root,
            priorities,
            git_roots,
            git_roots_limited,
            self.git_priorities.clone(),
            self.synthetic_roots.clone(),
            ignore,
        );
        self.status = WatchStatus::Scanning {
            generation: self.generation,
            watched: 0,
            examined: 0,
        };
        true
    }

    /// Apply one generation-tagged worker update.
    pub(crate) fn apply_discovery(&mut self, update: DiscoveryUpdate) -> bool {
        self.discovery.flush_pending();
        let generation = match &update {
            DiscoveryUpdate::Progress { generation, .. }
            | DiscoveryUpdate::Finished { generation, .. } => *generation,
        };
        if generation != self.generation {
            return false;
        }
        match update {
            DiscoveryUpdate::Progress {
                paths, examined, ..
            } => {
                self.root_dirs.extend(paths);
                self.status = WatchStatus::Scanning {
                    generation,
                    watched: self.root_dirs.len(),
                    examined,
                };
            }
            DiscoveryUpdate::Finished {
                watched,
                examined,
                outcome,
                ..
            } => {
                self.status = match outcome {
                    DiscoveryOutcome::Complete if self.control_limited => WatchStatus::Limited {
                        watched,
                        examined,
                        reason: LimitReason::ControlWatches,
                    },
                    DiscoveryOutcome::Complete => WatchStatus::Complete { watched, examined },
                    DiscoveryOutcome::Limited(reason) => WatchStatus::Limited {
                        watched,
                        examined,
                        reason,
                    },
                    DiscoveryOutcome::Errored(reason) => WatchStatus::Errored {
                        watched,
                        examined,
                        reason,
                    },
                    DiscoveryOutcome::Cancelled => return false,
                };
                self.synthetic_roots.clear();
                self.git_priorities.clear();
            }
        }
        true
    }

    #[must_use]
    pub(crate) fn status(&self) -> &WatchStatus {
        &self.status
    }

    /// Whether narrow state and Git surfaces need recovery.
    pub(crate) const fn coverage_complete(&self) -> bool {
        self.coverage.complete()
    }

    /// Whether a raw event came from one of the requested surfaces.
    pub(crate) fn accepts(&self, raw: &Raw) -> bool {
        matches!(raw, Raw::Rescan) || raw.paths().any(|path| self.is_target(path))
    }

    /// Whether an event on `path` is one this watcher was asked for.
    pub(crate) fn is_target(&self, path: &Path) -> bool {
        self.state_files.contains(path)
            || self.state_dirs.contains(path)
            || self.targets.contains(path)
            || self.targets.iter().any(|target| target.starts_with(path))
            || self.extras.iter().any(|dir| path.starts_with(dir))
            || self
                .discovery
                .git_roots
                .iter()
                .any(|root| path.starts_with(root))
            || self
                .synthetic_roots
                .iter()
                .any(|root| path.starts_with(root))
            || path.parent().is_some_and(|parent| {
                self.root_dirs.contains(parent) || self.priority_dirs.contains(parent)
            })
    }

    /// Reprioritize bounded discovery for a new directory under Git refs.
    fn discover_git_created(&mut self, path: &Path) -> bool {
        if !self
            .discovery
            .git_roots
            .iter()
            .any(|root| path.starts_with(root))
            || !path
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
        {
            return false;
        }
        let Some(root) = self.root.clone() else {
            return false;
        };
        self.git_priorities.retain(|priority| priority != path);
        self.git_priorities.insert(0, path.to_path_buf());
        self.git_priorities.truncate(self.limits.retained_paths());
        self.root_dirs.clear();
        let priorities = self.discovery.priorities.clone();
        let git_roots = self.discovery.git_roots.clone();
        let git_roots_limited = self.discovery.git_roots_limited;
        let ignore = self.discovery.ignore.clone();
        self.generation = self.discovery.request(
            root,
            priorities,
            git_roots,
            git_roots_limited,
            self.git_priorities.clone(),
            self.synthetic_roots.clone(),
            ignore,
        );
        self.status = WatchStatus::Scanning {
            generation: self.generation,
            watched: 0,
            examined: 0,
        };
        true
    }

    /// Watch immediate Git metadata and move refs/registry trees to the worker.
    pub(crate) fn watch_worktrees(&mut self, paths: &[PathBuf]) -> bool {
        let examined = paths.len().min(CONTROL_WATCH_LIMIT);
        let mut control_limited = paths.len() > examined;
        let mut git_roots = Vec::with_capacity(examined);
        let mut wanted = Vec::with_capacity(examined);
        let mut seen = HashSet::with_capacity(examined);
        let mut git_roots_limited = false;
        for path in paths.iter().take(examined) {
            if !seen.insert(path.clone()) {
                continue;
            }
            if is_recursive_git_root(path) {
                if git_roots.len() == self.limits.retained_paths() {
                    git_roots_limited = true;
                    continue;
                }
                git_roots.push(path.clone());
            } else {
                wanted.push(path.clone());
            }
        }
        let restart_discovery = git_roots != self.discovery.git_roots
            || git_roots_limited != self.discovery.git_roots_limited;
        let old = std::mem::take(&mut self.extras);
        self.extras = wanted.iter().cloned().collect();
        for stale in old {
            if !self.extras.contains(&stale) {
                let _ = self.reconcile(&stale);
            }
        }
        let mut complete = true;
        for path in wanted {
            if !self.watched.contains(&path) && self.watched.len() >= CONTROL_WATCH_LIMIT {
                control_limited = true;
                continue;
            }
            if let Err(error) = self.reconcile(&path) {
                tracing::debug!(%error, dir = %path.display(), "cannot watch worktree path");
                self.extras.remove(&path);
                complete = false;
            }
        }
        self.coverage.set(Surface::Extras, complete);
        self.set_control_limited(control_limited);
        if restart_discovery && let Some(root) = self.root.clone() {
            let priorities = self.discovery.priorities.clone();
            let ignore = self.discovery.ignore.clone();
            self.root_dirs.clear();
            self.synthetic_roots.clear();
            self.generation = self.discovery.request(
                root,
                priorities,
                git_roots,
                git_roots_limited,
                self.git_priorities.clone(),
                Vec::new(),
                ignore,
            );
            self.status = WatchStatus::Scanning {
                generation: self.generation,
                watched: 0,
                examined: 0,
            };
        }
        complete
    }

    fn set_control_limited(&mut self, limited: bool) {
        self.control_limited = limited;
        self.status = match (&self.status, limited) {
            (WatchStatus::Complete { watched, examined }, true) => WatchStatus::Limited {
                watched: *watched,
                examined: *examined,
                reason: LimitReason::ControlWatches,
            },
            (
                WatchStatus::Limited {
                    watched,
                    examined,
                    reason: LimitReason::ControlWatches,
                },
                false,
            ) => WatchStatus::Complete {
                watched: *watched,
                examined: *examined,
            },
            _ => return,
        };
    }

    /// Watch the directories holding exact state `files`.
    pub(crate) fn watch_state<'a>(&mut self, files: impl IntoIterator<Item = &'a Path>) -> bool {
        let old = std::mem::take(&mut self.state_dirs);
        let old_parents = std::mem::take(&mut self.state_parent_dirs);
        self.state_files = files.into_iter().map(Path::to_path_buf).collect();
        self.state_dirs = self
            .state_files
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect();
        self.state_parent_dirs = self
            .state_dirs
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect();
        let mut complete = true;
        let wanted: Vec<PathBuf> = self
            .state_dirs
            .iter()
            .chain(&self.state_parent_dirs)
            .cloned()
            .collect();
        for stale in old.into_iter().chain(old_parents) {
            if !self.state_dirs.contains(&stale) && !self.state_parent_dirs.contains(&stale) {
                let _ = self.reconcile(&stale);
            }
        }
        for dir in wanted {
            if !dir.is_dir() {
                self.forget_watch(&dir);
                complete = false;
                continue;
            }
            if !self.watched.contains(&dir) && self.watched.len() >= CONTROL_WATCH_LIMIT {
                let evicted = self.extras.iter().find(|path| {
                    self.watched.contains(*path)
                        && !self.state_dirs.contains(*path)
                        && !self.state_parent_dirs.contains(*path)
                });
                if let Some(evicted) = evicted.cloned() {
                    self.forget_watch(&evicted);
                    self.set_control_limited(true);
                } else {
                    complete = false;
                    continue;
                }
            }
            if let Err(error) = self.reconcile(&dir) {
                tracing::warn!(%error, dir = %dir.display(), "cannot watch workspace state");
                complete = false;
                continue;
            }
            if !dir.is_dir() {
                self.forget_watch(&dir);
                complete = false;
            }
        }
        self.coverage.set(Surface::State, complete);
        complete
    }

    /// Whether `raw` can change the exact thread store or its directory.
    pub(crate) fn thread_state_dirty(&self, raw: &Raw) -> bool {
        let exact = |path: &Path| {
            self.state_files.contains(path)
                || self.state_dirs.contains(path)
                || self.state_parent_dirs.contains(path)
        };
        match raw {
            Raw::Rescan => true,
            Raw::Modify(path) => {
                self.state_files.contains(path) || self.state_parent_dirs.contains(path)
            }
            Raw::Create(path) | Raw::Remove(path) | Raw::RenameFrom(path) | Raw::RenameTo(path) => {
                exact(path)
            }
            Raw::Rename { from, to } => exact(from) || exact(to),
        }
    }

    /// Drop cached state-watch installation after loss or replacement.
    ///
    /// `notify` may silently discard a watch when its directory is moved
    /// or removed. Keeping the path in `watched` would then make a retry a
    /// false success.
    pub(crate) fn invalidate_state_watches(&mut self, raw: &Raw) -> bool {
        let structural_path =
            |path: &Path| self.state_dirs.contains(path) || self.state_parent_dirs.contains(path);
        let parent_invalid = match raw {
            Raw::Create(path) | Raw::Remove(path) | Raw::RenameFrom(path) | Raw::RenameTo(path) => {
                self.state_parent_dirs.contains(path)
            }
            Raw::Rename { from, to } => {
                self.state_parent_dirs.contains(from) || self.state_parent_dirs.contains(to)
            }
            Raw::Modify(path) => self.state_parent_dirs.contains(path),
            Raw::Rescan => false,
        };
        let invalid = matches!(raw, Raw::Rescan)
            || match raw {
                Raw::Rescan => true,
                Raw::Create(path)
                | Raw::Remove(path)
                | Raw::RenameFrom(path)
                | Raw::RenameTo(path) => structural_path(path),
                Raw::Rename { from, to } => structural_path(from) || structural_path(to),
                Raw::Modify(path) => self.state_parent_dirs.contains(path),
            };
        if !invalid {
            return false;
        }
        let paths: Vec<PathBuf> = if matches!(raw, Raw::Rescan) || parent_invalid {
            self.state_dirs
                .iter()
                .chain(&self.state_parent_dirs)
                .cloned()
                .collect()
        } else {
            self.state_dirs.iter().cloned().collect()
        };
        for path in paths {
            if self.watched.remove(&path) {
                let _ = self.inner.unwatch(&path);
            }
        }
        self.coverage.set(Surface::State, false);
        true
    }

    fn wants_narrow_watch(&self, path: &Path) -> bool {
        self.extras.contains(path)
            || self.state_dirs.contains(path)
            || self.state_parent_dirs.contains(path)
    }

    fn reconcile(&mut self, path: &Path) -> notify::Result<()> {
        let wanted = self.wants_narrow_watch(path);
        let have = self.watched.contains(path);
        if wanted == have {
            return Ok(());
        }
        if have {
            let _ = self.inner.unwatch(path);
            self.watched.remove(path);
        }
        if !wanted {
            return Ok(());
        }
        self.inner.watch(path, RecursiveMode::NonRecursive)?;
        self.watched.insert(path.to_path_buf());
        Ok(())
    }

    fn forget_watch(&mut self, path: &Path) {
        if self.watched.remove(path) {
            let _ = self.inner.unwatch(path);
        }
    }
}

fn is_recursive_git_root(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "refs" || name == "worktrees")
}

impl App {
    /// Start or reprioritize broad coverage without doing recursive work here.
    pub(crate) fn sync_workspace_watches(&self, watcher: &mut Watcher, force: bool) -> bool {
        let root = self.workspace.root().to_path_buf();
        let loaded = self.loaded_abs_paths(watcher.limits.workspace_watches());
        watcher.follow(loaded.iter().map(PathBuf::as_path));
        watcher.discover(
            root,
            self.workspace_watch_priorities(&loaded, watcher.limits.retained_paths()),
            self.ignore.clone(),
            force,
        )
    }

    /// Reprioritize discovery around a newly-created subtree.
    pub(crate) fn watch_created(&mut self, watcher: &mut Watcher, path: &Path) -> bool {
        if watcher.discover_git_created(path) {
            return true;
        }
        let root = self.workspace.root().to_path_buf();
        let Ok(relative) = path.strip_prefix(&root) else {
            return false;
        };
        if !path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
            || self.ignore.is_tree_ignored(relative)
            || self.workspace.is_ignored(relative, EntryKind::Dir)
        {
            return false;
        }
        let loaded = self.loaded_abs_paths(watcher.limits.workspace_watches());
        watcher.follow(loaded.iter().map(PathBuf::as_path));
        watcher.discover_created(
            self.workspace_watch_priorities(&loaded, watcher.limits.retained_paths()),
            self.ignore.clone(),
            path.to_path_buf(),
        )
    }

    /// Absolute paths of every document retained by this viewer.
    pub(crate) fn loaded_abs_paths(&self, limit: usize) -> Vec<PathBuf> {
        let root = self.workspace.root();
        self.docs
            .iter()
            .take(limit)
            .map(|doc| root.join(&doc.relative))
            .collect()
    }

    fn workspace_watch_priorities(&self, loaded: &[PathBuf], limit: usize) -> Vec<PathBuf> {
        let root = self.workspace.root();
        let mut priorities = Vec::new();
        let mut seen = HashSet::new();
        for parent in loaded.iter().filter_map(|path| path.parent()) {
            if !parent.starts_with(root) {
                if seen.insert(parent.to_path_buf()) {
                    priorities.push(parent.to_path_buf());
                    if priorities.len() == limit {
                        return priorities;
                    }
                }
                continue;
            }
            let mut ancestors: Vec<PathBuf> = parent
                .ancestors()
                .take_while(|path| path.starts_with(root))
                .map(Path::to_path_buf)
                .collect();
            ancestors.reverse();
            for path in ancestors {
                if seen.insert(path.clone()) {
                    priorities.push(path);
                    if priorities.len() == limit {
                        return priorities;
                    }
                }
            }
        }
        if let Some(tree) = self.tree.as_ref() {
            for row in tree.rows().iter().filter(|row| row.is_dir()) {
                let path = root.join(row.path());
                if seen.insert(path.clone()) {
                    priorities.push(path);
                    if priorities.len() == limit {
                        return priorities;
                    }
                }
            }
        }
        priorities
    }

    /// Whether a raw event can affect observable workspace state.
    ///
    /// State and Git paths have already been narrowed by [`Watcher`].
    /// Within the worktree, ignored noise is dropped before it reaches the
    /// debounce queue. An explicitly loaded ignored file and its ancestors
    /// remain relevant so the open document still reloads and follows moves.
    pub(crate) fn raw_is_relevant(&mut self, raw: &Raw) -> bool {
        if matches!(raw, Raw::Rescan) {
            return true;
        }
        let root = self.workspace.root().to_path_buf();
        raw.paths().any(|path| {
            let Ok(relative) = path.strip_prefix(&root) else {
                return true;
            };
            if relative.starts_with(".git") || is_rules_file(relative) {
                return true;
            }
            if self
                .docs
                .iter()
                .any(|doc| doc.relative == relative || doc.relative.starts_with(relative))
            {
                return true;
            }
            if self.ignore.is_ignored(relative) {
                return false;
            }
            let kind = if path.is_dir() {
                EntryKind::Dir
            } else {
                EntryKind::File
            };
            !self.workspace.is_ignored(relative, kind)
        })
    }
}

/// Whether a path under `.git` can move HEAD or the index. Object
/// writes, reflogs, and lock files churn constantly and change neither.
pub(crate) fn is_git_metadata(relative: &Path) -> bool {
    // Lock files (`HEAD.lock`, `index.lock`) fall through the exact match.
    relative
        .components()
        .nth(1)
        .and_then(|c| c.as_os_str().to_str())
        .is_some_and(|first| {
            matches!(
                first,
                "HEAD" | "ORIG_HEAD" | "index" | "packed-refs" | "refs"
            )
        })
}

/// Raw events waiting out the hint debounce (ADR 0015), folded into
/// classified [`Event`]s when it ends.
#[derive(Debug)]
pub(crate) struct Batch {
    raw: Vec<Raw>,
    flush_at: Option<Instant>,
    limit: usize,
}

impl Batch {
    #[must_use]
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            raw: Vec::with_capacity(limit.min(256)),
            flush_at: None,
            limit: limit.max(1),
        }
    }

    /// Adds an event and restarts the quiet period.
    pub(crate) fn push(&mut self, raw: Raw, debounce: Duration) {
        if self.raw == [Raw::Rescan] {
            return;
        }
        if matches!(raw, Raw::Rescan) || self.raw.len() >= self.limit {
            self.raw.clear();
            self.raw.push(Raw::Rescan);
            self.flush_at
                .get_or_insert_with(|| Instant::now() + debounce);
            return;
        }
        self.raw.push(raw);
        self.flush_at = Some(Instant::now() + debounce);
    }

    /// Resolves once the quiet period ends; never while the batch is empty.
    pub(crate) async fn settled(&self) {
        match self.flush_at {
            Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
            None => std::future::pending().await,
        }
    }

    /// Fold the batch into one event per path. `previous_fingerprint` gives the
    /// fingerprint a removed path last had, for pairing an unpaired
    /// remove-then-create as a rename; a created file over `max_bytes`
    /// is not read for one.
    pub(crate) fn take(
        &mut self,
        previous_fingerprint: impl Fn(&Path) -> Option<Fingerprint>,
        max_bytes: u64,
    ) -> Vec<Event> {
        self.flush_at = None;
        let raw = std::mem::take(&mut self.raw);
        classify(&raw, previous_fingerprint, max_bytes)
    }
}

impl Default for Batch {
    fn default() -> Self {
        Self::new(WatchLimits::default().pending_events())
    }
}

/// Fold raw events into one [`Event`] per path, in first-seen order.
///
/// Platform-paired renames win; a `RenameFrom` then a `RenameTo` in the
/// batch are paired in order (inotify reports them so and then repeats
/// them as a pair, which is folded away); a rename whose old name is
/// created again in the batch is an editor's backup swap, a change to
/// that name; what is left is created, removed, or changed by the last
/// thing that happened to the path, and a removed path whose previous
/// fingerprint matches a created file is a rename after all. A created file over `max_bytes` is never read
/// for its fingerprint, so a large drop costs the loop nothing.
fn classify(
    raw: &[Raw],
    previous_fingerprint: impl Fn(&Path) -> Option<Fingerprint>,
    max_bytes: u64,
) -> Vec<Event> {
    if raw.contains(&Raw::Rescan) {
        return vec![Event::Rescan];
    }
    let mut renames: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut pending_from: Option<PathBuf> = None;
    let mut order: Vec<PathBuf> = Vec::new();
    // The last existence-changing thing seen per path.
    let mut fate: HashMap<PathBuf, Fate> = HashMap::new();
    let mut note = |path: &Path, f: Fate, order: &mut Vec<PathBuf>| {
        if !fate.contains_key(path) {
            order.push(path.to_path_buf());
        }
        let entry = fate.entry(path.to_path_buf()).or_insert(f);
        *entry = entry.then(f);
    };
    for event in raw {
        match event {
            Raw::Rescan => unreachable!("rescan events return before path classification"),
            Raw::Rename { from, to } => {
                pending_from.take();
                renames.push((from.clone(), to.clone()));
            }
            Raw::RenameFrom(from) => {
                if let Some(orphan) = pending_from.replace(from.clone()) {
                    note(&orphan, Fate::Removed, &mut order);
                }
            }
            Raw::RenameTo(to) => match pending_from.take() {
                Some(from) => renames.push((from, to.clone())),
                None => note(to, Fate::Created, &mut order),
            },
            Raw::Create(path) | Raw::Remove(path) | Raw::Modify(path) => {
                // A rename's two halves arrive back to back; anything in
                // between means the `To` went outside the watch.
                if let Some(orphan) = pending_from.take() {
                    note(&orphan, Fate::Removed, &mut order);
                }
                let f = match event {
                    Raw::Create(_) => Fate::Created,
                    Raw::Remove(_) => Fate::Removed,
                    _ => Fate::Changed,
                };
                note(path, f, &mut order);
            }
        }
    }
    if let Some(orphan) = pending_from {
        note(&orphan, Fate::Removed, &mut order);
    }
    // inotify emits `From`, `To`, and then the pair; keep one rename and
    // drop the loose ends that belong to it.
    renames.dedup();
    renames.retain(|(from, to)| !unswap(from, to, &mut fate, &mut order));
    for (from, to) in &renames {
        fate.remove(from);
        fate.remove(to);
    }
    // An unpaired remove-then-create with matching content is a rename.
    let removed: Vec<PathBuf> = order
        .iter()
        .filter(|p| fate.get(*p) == Some(&Fate::Removed))
        .cloned()
        .collect();
    let created: Vec<PathBuf> = order
        .iter()
        .filter(|p| fate.get(*p) == Some(&Fate::Created))
        .cloned()
        .collect();
    if !removed.is_empty() && !created.is_empty() {
        let mut created_prints: Vec<(PathBuf, Fingerprint)> = created
            .into_iter()
            .filter_map(|p| Fingerprint::read(&p, max_bytes).map(|f| (p, f)))
            .collect();
        for from in removed {
            let Some(previous) = previous_fingerprint(&from) else {
                continue;
            };
            if let Some(index) = created_prints
                .iter()
                .position(|(_, fingerprint)| *fingerprint == previous)
            {
                let (to, _) = created_prints.swap_remove(index);
                fate.remove(&from);
                fate.remove(&to);
                renames.push((from, to));
            }
        }
    }
    let mut out: Vec<Event> = renames
        .into_iter()
        .map(|(from, to)| Event::Renamed { from, to })
        .collect();
    out.extend(order.into_iter().filter_map(|path| {
        let event = match fate.get(&path)? {
            Fate::Created => Event::Created(path),
            Fate::Removed => Event::Removed(path),
            Fate::Changed => Event::Change(path),
        };
        Some(event)
    }));
    out
}

/// Whether the rename `from` to `to` is an editor's backup swap (Helix
/// and Vim move the file aside, write a new one at its path, and delete
/// the backup): `from` was created again in the same batch. A swap is
/// folded into `fate` as a change to `from`; `to` is a fresh file whose
/// own fate stands, a removal folding away to nothing.
fn unswap(
    from: &Path,
    to: &Path,
    fate: &mut HashMap<PathBuf, Fate>,
    order: &mut Vec<PathBuf>,
) -> bool {
    if fate.get(from) != Some(&Fate::Created) {
        return false;
    }
    fate.insert(from.to_path_buf(), Fate::Changed);
    if fate.get(to) == Some(&Fate::Removed) {
        fate.remove(to);
    } else if !fate.contains_key(to) {
        fate.insert(to.to_path_buf(), Fate::Created);
        order.push(to.to_path_buf());
    }
    true
}

/// What a burst did to one path, folded in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    Created,
    Removed,
    Changed,
}

impl Fate {
    /// The outcome of `self` followed by `next`.
    fn then(self, next: Self) -> Self {
        match (self, next) {
            // A write to a new file is still a new file; a file that came
            // and went is gone; a removed file written again is back.
            (Self::Created, Self::Changed) | (Self::Removed, Self::Changed | Self::Created) => {
                Self::Created
            }
            (_, next) => next,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use anyhow::Context as _;
    use fathomable_core::follow::Ignore;
    use fathomable_testing::{TempDir, git};
    use notify::EventKind;
    use notify::event::{
        AccessKind, AccessMode, CreateKind, Flag, ModifyKind, RemoveKind, RenameMode,
    };

    use crate::app::testing::AppBuilder;

    use super::{
        Batch, CONTROL_WATCH_LIMIT, Event, Fingerprint, LimitReason, Raw, WatchLimits, WatchStatus,
        Watcher, classify, is_git_metadata,
    };

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn no_snapshot(_: &Path) -> Option<Fingerprint> {
        None
    }

    async fn settle(
        watcher: &mut Watcher,
        discoveries: &mut super::Discoveries,
    ) -> anyhow::Result<()> {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let update = discoveries
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("discovery worker stopped"))?;
                watcher.apply_discovery(update);
                if !matches!(watcher.status(), WatchStatus::Scanning { .. }) {
                    return Ok::<(), anyhow::Error>(());
                }
            }
        })
        .await??;
        Ok(())
    }

    #[test]
    fn watcher_keeps_writes_and_drops_reads() {
        let event = |kind| notify::Event::new(kind).add_path(p("/w/a"));
        assert_eq!(
            Raw::from_notify(event(EventKind::Create(CreateKind::File))),
            [Raw::Create(p("/w/a"))]
        );
        assert_eq!(
            Raw::from_notify(event(EventKind::Modify(ModifyKind::Any))),
            [Raw::Modify(p("/w/a"))]
        );
        assert_eq!(
            Raw::from_notify(event(EventKind::Remove(RemoveKind::File))),
            [Raw::Remove(p("/w/a"))]
        );
        assert!(
            Raw::from_notify(event(EventKind::Access(AccessKind::Open(AccessMode::Read))))
                .is_empty()
        );
        assert!(Raw::from_notify(event(EventKind::Any)).is_empty());
        let rescan = notify::Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert_eq!(Raw::from_notify(rescan), [Raw::Rescan]);
        let both = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path(p("/w/a"))
            .add_path(p("/w/b"));
        assert_eq!(
            Raw::from_notify(both),
            [Raw::Rename {
                from: p("/w/a"),
                to: p("/w/b")
            }]
        );
    }

    #[test]
    fn git_metadata_is_head_index_and_refs() {
        assert!(is_git_metadata(Path::new(".git/HEAD")));
        assert!(is_git_metadata(Path::new(".git/index")));
        assert!(is_git_metadata(Path::new(".git/refs/heads/main")));
        assert!(is_git_metadata(Path::new(".git/packed-refs")));
        assert!(!is_git_metadata(Path::new(".git/index.lock")));
        assert!(!is_git_metadata(Path::new(".git/objects/ab/cdef")));
        assert!(!is_git_metadata(Path::new(".git/logs/HEAD")));
        assert!(!is_git_metadata(Path::new(".git")));
    }

    #[tokio::test]
    async fn workspace_watches_cover_visible_directories_not_build_trees() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-visible")?;
        git::init(&dir.0)?;
        fs::write(dir.0.join(".gitignore"), "target/\n.tmp/\n")?;
        fs::create_dir_all(dir.0.join("src/nested"))?;
        fs::create_dir_all(dir.0.join("target/deep/cache"))?;
        fs::create_dir_all(dir.0.join(".tmp/worktrees/generated"))?;
        fs::create_dir_all(dir.0.join("build/deep"))?;
        let ignore = Ignore::new(&["build/**".to_owned()])?;
        let (mut watcher, _) = Watcher::new()?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;

        assert!(watcher.discover(dir.0.clone(), Vec::new(), ignore, false));
        settle(&mut watcher, &mut discoveries).await?;
        assert!(matches!(watcher.status(), WatchStatus::Complete { .. }));
        assert_eq!(
            watcher.root_dirs,
            [dir.0.clone(), dir.0.join("src"), dir.0.join("src/nested"),]
                .into_iter()
                .collect()
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_new_visible_subtree_is_watched_before_its_files_are_reported() -> anyhow::Result<()>
    {
        let dir = TempDir::new("watch-created")?;
        git::init(&dir.0)?;
        fs::create_dir_all(dir.0.join("src"))?;
        let ignore = Ignore::default();
        let (mut watcher, mut raws) = Watcher::new()?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        assert!(watcher.discover(dir.0.clone(), Vec::new(), ignore.clone(), false));
        settle(&mut watcher, &mut discoveries).await?;

        fs::create_dir_all(dir.0.join("src/new/deep"))?;
        fs::write(dir.0.join("src/new/deep/file.rs"), "fn main() {}\n")?;
        assert!(watcher.discover_created(
            vec![dir.0.join("src/new")],
            ignore,
            dir.0.join("src/new")
        ));
        settle(&mut watcher, &mut discoveries).await?;
        assert!(watcher.root_dirs.contains(&dir.0.join("src/new")));
        assert!(watcher.root_dirs.contains(&dir.0.join("src/new/deep")));
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if raws.recv().await == Some(Raw::Create(dir.0.join("src/new/deep/file.rs"))) {
                    break;
                }
            }
        })
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn loaded_ignored_files_receive_priority_coverage() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-open-ignored")?;
        git::init(&dir.0)?;
        fs::write(dir.0.join(".gitignore"), "target/\n")?;
        fs::create_dir_all(dir.0.join("target/one"))?;
        fs::create_dir_all(dir.0.join("target/two"))?;
        let first = dir.0.join("target/one/output.txt");
        let second = dir.0.join("target/two/output.txt");
        fs::write(&first, "one\n")?;
        fs::write(&second, "two\n")?;
        let (mut watcher, _) = Watcher::new()?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.follow([first.as_path(), second.as_path()]);
        assert!(watcher.discover(
            dir.0.clone(),
            vec![dir.0.join("target/one"), dir.0.join("target/two")],
            Ignore::default(),
            false,
        ));
        settle(&mut watcher, &mut discoveries).await?;

        assert!(watcher.root_dirs.contains(&dir.0.join("target")));
        assert!(watcher.root_dirs.contains(&dir.0.join("target/one")));
        assert!(watcher.root_dirs.contains(&dir.0.join("target/two")));
        assert!(watcher.accepts(&Raw::Modify(first)));
        assert!(watcher.accepts(&Raw::Modify(second)));
        Ok(())
    }

    #[tokio::test]
    async fn loaded_file_through_directory_symlink_watches_only_its_parent() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-loaded-symlink")?;
        let root = dir.0.join("workspace");
        let outside = dir.0.join("outside");
        fs::create_dir(&root)?;
        fs::create_dir(&outside)?;
        fs::create_dir(outside.join("unopened"))?;
        let loaded_file = outside.join("loaded.md");
        fs::write(&loaded_file, "loaded\n")?;
        symlink(&outside, root.join("linked"))?;
        let mut app = AppBuilder::at(&root).unopened().build()?;
        app.open(Path::new("linked/loaded.md"));
        assert_eq!(app.loaded_abs_paths(1), vec![loaded_file.clone()]);

        let limits = WatchLimits::new(8, 100, 100, 16).context("positive limits")?;
        let (mut watcher, _) = Watcher::with_limits(limits)?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        assert!(app.sync_workspace_watches(&mut watcher, false));
        settle(&mut watcher, &mut discoveries).await?;

        assert!(watcher.root_dirs.contains(&outside));
        assert!(!watcher.root_dirs.contains(&outside.join("unopened")));
        assert!(watcher.accepts(&Raw::Modify(loaded_file)));
        Ok(())
    }

    #[tokio::test]
    async fn small_watch_cap_is_truthfully_limited() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-small-cap")?;
        for index in 0..8 {
            fs::create_dir(dir.0.join(format!("dir-{index}")))?;
        }
        let limits = WatchLimits::new(3, 100, 100, 16).context("positive limits")?;
        let (mut watcher, _) = Watcher::with_limits(limits)?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(dir.0.clone(), Vec::new(), Ignore::default(), false);
        settle(&mut watcher, &mut discoveries).await?;
        assert!(matches!(
            watcher.status(),
            WatchStatus::Limited {
                watched: 3,
                reason: LimitReason::WorkspaceWatches,
                ..
            }
        ));
        assert_eq!(watcher.root_dirs.len(), 3);
        Ok(())
    }

    #[tokio::test]
    async fn priority_path_wins_under_small_cap() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-priority")?;
        fs::create_dir_all(dir.0.join("aaa/noise"))?;
        fs::create_dir_all(dir.0.join("wanted/deep"))?;
        let limits = WatchLimits::new(3, 100, 100, 16).context("positive limits")?;
        let (mut watcher, _) = Watcher::with_limits(limits)?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(
            dir.0.clone(),
            vec![dir.0.join("wanted/deep")],
            Ignore::default(),
            false,
        );
        settle(&mut watcher, &mut discoveries).await?;
        assert!(watcher.root_dirs.contains(&dir.0));
        assert!(watcher.root_dirs.contains(&dir.0.join("wanted")));
        assert!(watcher.root_dirs.contains(&dir.0.join("wanted/deep")));
        assert!(!watcher.root_dirs.contains(&dir.0.join("aaa")));
        Ok(())
    }

    #[tokio::test]
    async fn priority_is_watched_before_root_exhausts_entry_budget() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-priority-entry-cap")?;
        git::init(&dir.0)?;
        fs::write(dir.0.join(".gitignore"), "ignored/\n")?;
        for index in 0..10 {
            fs::create_dir(dir.0.join(format!("noise-{index}")))?;
        }
        let priority = dir.0.join("ignored/deep");
        fs::create_dir_all(&priority)?;
        let limits = WatchLimits::new(20, 4, 100, 16).context("positive limits")?;
        let (mut watcher, _) = Watcher::with_limits(limits)?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(
            dir.0.clone(),
            vec![priority.clone()],
            Ignore::default(),
            false,
        );
        settle(&mut watcher, &mut discoveries).await?;

        assert!(watcher.root_dirs.contains(&dir.0));
        assert!(watcher.root_dirs.contains(&dir.0.join("ignored")));
        assert!(watcher.root_dirs.contains(&priority));
        assert!(matches!(
            watcher.status(),
            WatchStatus::Limited {
                reason: LimitReason::DiscoveryEntries,
                ..
            }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn immediate_git_controls_are_capped_without_retries() -> anyhow::Result<()> {
        let workspace = TempDir::new("watch-control-workspace")?;
        let dir = TempDir::new("watch-control-cap")?;
        let mut controls = Vec::new();
        for index in 0..(CONTROL_WATCH_LIMIT + 8) {
            let control = dir.0.join(format!("control-{index:02}"));
            fs::create_dir(&control)?;
            controls.push(control);
        }
        let limits = WatchLimits::new(4, 100, 100, 16).context("positive limits")?;
        let (mut watcher, _) = Watcher::with_limits(limits)?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(workspace.0.clone(), Vec::new(), Ignore::default(), false);
        settle(&mut watcher, &mut discoveries).await?;

        assert!(watcher.watch_worktrees(&controls));
        assert_eq!(watcher.watched.len(), CONTROL_WATCH_LIMIT);
        assert!(
            controls[..CONTROL_WATCH_LIMIT]
                .iter()
                .all(|path| watcher.watched.contains(path))
        );
        assert!(
            controls[CONTROL_WATCH_LIMIT..]
                .iter()
                .all(|path| !watcher.watched.contains(path))
        );
        assert!(watcher.coverage_complete(), "a hard cap is not retryable");
        assert!(matches!(
            watcher.status(),
            WatchStatus::Limited {
                reason: LimitReason::ControlWatches,
                ..
            }
        ));

        let state = dir.0.join("state");
        fs::create_dir(&state)?;
        let state_file = state.join("threads.jsonl");
        assert!(watcher.watch_state([state_file.as_path()]));
        assert_eq!(watcher.watched.len(), CONTROL_WATCH_LIMIT);
        assert!(watcher.watched.contains(&state));
        assert!(watcher.watched.contains(&dir.0));
        assert!(watcher.coverage_complete());
        Ok(())
    }

    #[tokio::test]
    async fn nested_git_refs_are_installed_by_the_bounded_worker() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-git-refs")?;
        let refs = dir.0.join(".git/refs");
        let nested = refs.join("heads/team");
        fs::create_dir_all(&nested)?;
        fs::create_dir_all(dir.0.join("workspace-noise"))?;
        let limits = WatchLimits::new(4, 100, 100, 16).context("positive limits")?;
        let (mut watcher, mut raws) = Watcher::with_limits(limits)?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(dir.0.clone(), Vec::new(), Ignore::default(), false);
        watcher.watch_worktrees(std::slice::from_ref(&refs));
        settle(&mut watcher, &mut discoveries).await?;

        assert!(watcher.root_dirs.contains(&refs));
        assert!(watcher.root_dirs.contains(&refs.join("heads")));
        assert!(watcher.root_dirs.contains(&nested));
        assert!(
            !watcher.watched.contains(&refs),
            "recursive refs never reach the event-loop-owned watcher"
        );
        assert!(matches!(
            watcher.status(),
            WatchStatus::Limited {
                reason: LimitReason::WorkspaceWatches,
                ..
            }
        ));

        let reference = nested.join("topic");
        fs::write(&reference, "synthetic-ref\n")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let raw = raws
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("watcher channel closed"))?;
                if raw.paths().any(|path| path == reference) {
                    assert!(watcher.accepts(&raw));
                    break Ok::<(), anyhow::Error>(());
                }
            }
        })
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn linked_worktree_heads_are_installed_from_registry_anchor() -> anyhow::Result<()> {
        let workspace = TempDir::new("watch-registry-workspace")?;
        let git = TempDir::new("watch-registry-git")?;
        let registry = git.0.join("worktrees");
        let linked = registry.join("linked");
        fs::create_dir_all(&linked)?;
        let limits = WatchLimits::new(6, 100, 100, 16).context("positive limits")?;
        let (mut watcher, mut raws) = Watcher::with_limits(limits)?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(workspace.0.clone(), Vec::new(), Ignore::default(), false);
        watcher.watch_worktrees(std::slice::from_ref(&registry));
        settle(&mut watcher, &mut discoveries).await?;

        assert!(watcher.root_dirs.contains(&registry));
        assert!(watcher.root_dirs.contains(&linked));
        assert!(!watcher.watched.contains(&registry));

        let head = linked.join("HEAD");
        fs::write(&head, "ref: refs/heads/feature\n")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let raw = raws
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("watcher channel closed"))?;
                if raw.paths().any(|path| path == head) {
                    assert!(watcher.accepts(&raw));
                    break Ok::<(), anyhow::Error>(());
                }
            }
        })
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn created_external_ref_namespace_gets_nested_coverage() -> anyhow::Result<()> {
        let workspace = TempDir::new("watch-linked-worktree")?;
        let mut app = AppBuilder::at(&workspace.0).unopened().build()?;
        let git = TempDir::new("watch-linked-git")?;
        let refs = git.0.join("refs");
        fs::create_dir(&refs)?;
        let limits = WatchLimits::new(10, 100, 100, 16).context("positive limits")?;
        let (mut watcher, mut raws) = Watcher::with_limits(limits)?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(workspace.0.clone(), Vec::new(), Ignore::default(), false);
        watcher.watch_worktrees(std::slice::from_ref(&refs));
        settle(&mut watcher, &mut discoveries).await?;

        let namespace = refs.join("heads/team");
        fs::create_dir_all(&namespace)?;
        assert!(app.watch_created(&mut watcher, &namespace));
        settle(&mut watcher, &mut discoveries).await?;
        assert!(watcher.root_dirs.contains(&namespace));

        let reference = namespace.join("topic");
        fs::write(&reference, "one\n")?;
        fs::write(&reference, "two\n")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let raw = raws
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("watcher channel closed"))?;
                if raw.paths().any(|path| path == reference) {
                    assert!(watcher.accepts(&raw));
                    break Ok::<(), anyhow::Error>(());
                }
            }
        })
        .await??;
        Ok(())
    }

    #[tokio::test]
    async fn deep_priority_expansion_stops_before_allocating_all_ancestors() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-priority-bound")?;
        let deep = (0..100).fold(dir.0.clone(), |path, index| path.join(index.to_string()));
        let limits = WatchLimits::new(20, 100, 3, 16).context("positive limits")?;
        let (mut watcher, _) = Watcher::with_limits(limits)?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(dir.0.clone(), vec![deep], Ignore::default(), false);
        settle(&mut watcher, &mut discoveries).await?;
        assert!(matches!(
            watcher.status(),
            WatchStatus::Limited {
                reason: LimitReason::RetainedPaths,
                ..
            }
        ));
        assert!(watcher.root_dirs.len() <= 3);
        Ok(())
    }

    #[tokio::test]
    async fn wide_directory_stops_while_counting_entries() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-wide")?;
        for index in 0..20 {
            fs::write(dir.0.join(format!("file-{index}")), "")?;
        }
        let limits = WatchLimits::new(20, 4, 20, 16).context("positive limits")?;
        let (mut watcher, _) = Watcher::with_limits(limits)?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(dir.0.clone(), Vec::new(), Ignore::default(), false);
        settle(&mut watcher, &mut discoveries).await?;
        assert!(matches!(
            watcher.status(),
            WatchStatus::Limited {
                examined: 4,
                reason: LimitReason::DiscoveryEntries,
                ..
            }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn retained_pending_paths_are_bounded() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-retained")?;
        for index in 0..20 {
            fs::create_dir(dir.0.join(format!("dir-{index}")))?;
        }
        let limits = WatchLimits::new(20, 100, 2, 16).context("positive limits")?;
        let (mut watcher, _) = Watcher::with_limits(limits)?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(dir.0.clone(), Vec::new(), Ignore::default(), false);
        settle(&mut watcher, &mut discoveries).await?;
        assert!(matches!(
            watcher.status(),
            WatchStatus::Limited {
                reason: LimitReason::RetainedPaths,
                ..
            }
        ));
        assert!(watcher.root_dirs.len() <= 2);
        Ok(())
    }

    #[tokio::test]
    async fn unreadable_directory_is_an_explicit_error() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-unreadable")?;
        let denied = dir.0.join("denied");
        fs::create_dir(&denied)?;
        fs::write(denied.join("secret"), "synthetic\n")?;
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o000))?;
        let (mut watcher, _) = Watcher::new()?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(dir.0.clone(), Vec::new(), Ignore::default(), false);
        settle(&mut watcher, &mut discoveries).await?;
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o700))?;
        assert!(matches!(watcher.status(), WatchStatus::Errored { .. }));
        Ok(())
    }

    #[tokio::test]
    async fn stale_generation_results_cannot_replace_current_coverage() -> anyhow::Result<()> {
        let first = TempDir::new("watch-stale-first")?;
        for index in 0..300 {
            fs::create_dir(first.0.join(format!("dir-{index}")))?;
        }
        let second = TempDir::new("watch-stale-second")?;
        fs::create_dir_all(second.0.join("current/deep"))?;
        let (mut watcher, _) = Watcher::new()?;
        let mut discoveries = watcher.take_discoveries().context("discoveries")?;
        watcher.discover(first.0.clone(), Vec::new(), Ignore::default(), false);
        watcher.discover(second.0.clone(), Vec::new(), Ignore::default(), true);
        settle(&mut watcher, &mut discoveries).await?;
        assert!(
            watcher
                .root_dirs
                .iter()
                .all(|path| path.starts_with(&second.0))
        );
        assert!(watcher.root_dirs.contains(&second.0.join("current/deep")));
        Ok(())
    }

    #[test]
    fn dropping_does_not_join_a_worker_blocked_on_results() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-drop")?;
        for index in 0..(super::DISCOVERY_CHUNK * (super::DISCOVERY_RESULTS + 2)) {
            fs::create_dir(dir.0.join(format!("dir-{index}")))?;
        }
        let limits = WatchLimits::new(2_000, 5_000, 2_000, 16).context("positive limits")?;
        let (mut watcher, _) = Watcher::with_limits(limits)?;
        watcher.discover(dir.0.clone(), Vec::new(), Ignore::default(), false);
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        drop(watcher);
        assert!(started.elapsed() < Duration::from_millis(100));
        Ok(())
    }

    #[test]
    fn debounce_batch_coalesces_an_event_storm_to_rescan() {
        let mut batch = Batch::new(3);
        for index in 0..100 {
            batch.push(
                Raw::Modify(PathBuf::from(format!("/workspace/{index}"))),
                std::time::Duration::from_secs(30),
            );
        }
        assert_eq!(batch.take(no_snapshot, u64::MAX), [Event::Rescan]);
    }

    #[tokio::test]
    async fn bounded_notification_queue_reports_overflow() {
        let (tx, rx) = tokio::sync::mpsc::channel(2);
        let overflowed = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let sender = super::RawSender {
            tx,
            overflowed: std::sync::Arc::clone(&overflowed),
        };
        let mut raws = super::Raws { rx, overflowed };
        sender.send(Raw::Modify(p("/workspace/a")));
        sender.send(Raw::Modify(p("/workspace/b")));
        sender.send(Raw::Modify(p("/workspace/c")));

        assert_eq!(raws.recv().await, Some(Raw::Rescan));
        assert_eq!(raws.recv().await, Some(Raw::Modify(p("/workspace/a"))));
    }

    #[test]
    fn state_watch_failure_marks_coverage_partial() -> anyhow::Result<()> {
        let (mut watcher, _) = Watcher::new()?;
        let missing = PathBuf::from("/fathomable-test-missing/state/threads.jsonl");
        assert!(!watcher.watch_state([missing.as_path()]));
        assert!(!watcher.coverage_complete());
        Ok(())
    }

    #[test]
    fn state_directory_loss_invalidates_only_stale_installed_watches() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-state-loss")?;
        let state = dir.0.join("state");
        fs::create_dir(&state)?;
        let file = state.join("threads.jsonl");
        let (mut watcher, _) = Watcher::new()?;
        assert!(watcher.watch_state([file.as_path()]));
        assert!(watcher.watched.contains(&state));
        assert!(watcher.watched.contains(&dir.0));
        assert!(
            !watcher.thread_state_dirty(&Raw::Modify(state.clone())),
            "state-directory metadata does not trigger store reload"
        );

        let removed = Raw::Remove(state.clone());
        assert!(watcher.thread_state_dirty(&removed));
        assert!(watcher.invalidate_state_watches(&removed));
        assert!(!watcher.watched.contains(&state));
        assert!(
            watcher.watched.contains(&dir.0),
            "the parent remains able to observe recreation"
        );
        assert!(!watcher.coverage_complete());

        fs::remove_dir(&state)?;
        assert!(!watcher.watch_state([file.as_path()]));
        fs::create_dir(&state)?;
        assert!(watcher.watch_state([file.as_path()]));
        assert!(watcher.watched.contains(&state));
        Ok(())
    }

    #[tokio::test]
    async fn real_watcher_observes_store_lifecycle_without_access_feedback() -> anyhow::Result<()> {
        async fn dirty(watcher: &mut Watcher, raws: &mut super::Raws) -> anyhow::Result<()> {
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let raw = raws
                        .recv()
                        .await
                        .ok_or_else(|| anyhow::anyhow!("watcher channel closed"))?;
                    if watcher.thread_state_dirty(&raw) {
                        watcher.invalidate_state_watches(&raw);
                        return Ok::<(), anyhow::Error>(());
                    }
                }
            })
            .await??;
            Ok(())
        }

        let dir = TempDir::new("watch-state-smoke")?;
        let state = dir.0.join("state");
        fs::create_dir(&state)?;
        let file = state.join("threads.jsonl");
        let replacement = state.join("replacement");
        let (mut watcher, mut raws) = Watcher::new()?;
        assert!(watcher.watch_state([file.as_path()]));

        fs::write(&file, "one\n")?;
        dirty(&mut watcher, &mut raws).await?;
        assert!(watcher.watch_state([file.as_path()]));

        fs::write(&replacement, "two\n")?;
        fs::rename(&replacement, &file)?;
        dirty(&mut watcher, &mut raws).await?;

        fs::remove_file(&file)?;
        dirty(&mut watcher, &mut raws).await?;
        fs::remove_dir(&state)?;
        dirty(&mut watcher, &mut raws).await?;
        assert!(!watcher.watch_state([file.as_path()]));

        fs::create_dir(&state)?;
        dirty(&mut watcher, &mut raws).await?;
        assert!(watcher.watch_state([file.as_path()]));
        fs::write(&file, "three\n")?;
        dirty(&mut watcher, &mut raws).await?;
        Ok(())
    }

    #[tokio::test]
    async fn moved_state_parent_reinstalls_watches_on_recreated_tree() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-state-parent-move")?;
        let parent = dir.0.join("workspaces");
        let state = parent.join("workspace");
        fs::create_dir_all(&state)?;
        let file = state.join("threads.jsonl");
        fs::write(&file, "old\n")?;
        let moved = dir.0.join("moved-with-contents");
        let (mut watcher, mut raws) = Watcher::new()?;
        assert!(watcher.watch_state([file.as_path()]));

        fs::rename(&parent, &moved)?;
        let parent_event = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let raw = raws
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("watcher channel closed"))?;
                if raw.paths().any(|path| path == parent) && watcher.thread_state_dirty(&raw) {
                    break Ok::<Raw, anyhow::Error>(raw);
                }
            }
        })
        .await??;
        assert!(watcher.invalidate_state_watches(&parent_event));
        assert!(!watcher.watched.contains(&parent));
        assert!(!watcher.watched.contains(&state));
        assert!(!watcher.coverage_complete());
        assert_eq!(
            fs::read_to_string(moved.join("workspace/threads.jsonl"))?,
            "old\n"
        );

        fs::create_dir_all(&state)?;
        assert!(watcher.watch_state([file.as_path()]));
        fs::write(&file, "new\n")?;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let raw = raws
                    .recv()
                    .await
                    .ok_or_else(|| anyhow::anyhow!("watcher channel closed"))?;
                if raw.paths().any(|path| path == file) && watcher.thread_state_dirty(&raw) {
                    break Ok::<(), anyhow::Error>(());
                }
            }
        })
        .await??;
        Ok(())
    }

    #[test]
    fn a_burst_folds_to_one_event_per_path() {
        let events = classify(
            &[
                Raw::Create(p("/w/new")),
                Raw::Modify(p("/w/new")),
                Raw::Modify(p("/w/new")),
                Raw::Modify(p("/w/old")),
                Raw::Create(p("/w/tmp")),
                Raw::Remove(p("/w/tmp")),
                Raw::Remove(p("/w/gone")),
                Raw::Remove(p("/w/back")),
                Raw::Create(p("/w/back")),
            ],
            no_snapshot,
            u64::MAX,
        );
        assert_eq!(
            events,
            [
                Event::Created(p("/w/new")),
                Event::Change(p("/w/old")),
                Event::Removed(p("/w/tmp")),
                Event::Removed(p("/w/gone")),
                Event::Created(p("/w/back")),
            ]
        );
    }

    #[test]
    fn a_rescan_notice_supersedes_an_unreliable_batch() {
        let events = classify(
            &[
                Raw::Create(p("/w/new")),
                Raw::Rescan,
                Raw::Modify(p("/w/old")),
            ],
            no_snapshot,
            u64::MAX,
        );
        assert_eq!(events, [Event::Rescan]);
    }

    #[test]
    fn platform_rename_pairs_win_and_loose_ends_fold_away() {
        // inotify: From, To, then the pair it made of them.
        let events = classify(
            &[
                Raw::RenameFrom(p("/w/a")),
                Raw::RenameTo(p("/w/b")),
                Raw::Rename {
                    from: p("/w/a"),
                    to: p("/w/b"),
                },
                Raw::Modify(p("/w/b")),
            ],
            no_snapshot,
            u64::MAX,
        );
        assert_eq!(
            events,
            [Event::Renamed {
                from: p("/w/a"),
                to: p("/w/b")
            }]
        );
        // From then To without a pair are paired in order; a From whose
        // To never comes (moved out of the root) is a removal, and a To
        // without a From (moved in) is a creation.
        let events = classify(
            &[
                Raw::RenameFrom(p("/w/c")),
                Raw::RenameTo(p("/w/d")),
                Raw::RenameFrom(p("/w/out")),
                Raw::Create(p("/w/x")),
                Raw::RenameTo(p("/w/in")),
            ],
            no_snapshot,
            u64::MAX,
        );
        assert_eq!(
            events,
            [
                Event::Renamed {
                    from: p("/w/c"),
                    to: p("/w/d")
                },
                Event::Removed(p("/w/out")),
                Event::Created(p("/w/x")),
                Event::Created(p("/w/in")),
            ]
        );
    }

    #[test]
    fn an_editor_backup_swap_is_a_change_to_the_file() {
        // A Helix `:w` on inotify: the file moves aside to a backup, a
        // new file is written at its path, the backup goes (measured
        // with Helix 25.07).
        let events = classify(
            &[
                Raw::RenameFrom(p("/w/note.txt")),
                Raw::RenameTo(p("/w/note.txtzJmTMj.bck")),
                Raw::Rename {
                    from: p("/w/note.txt"),
                    to: p("/w/note.txtzJmTMj.bck"),
                },
                Raw::Create(p("/w/note.txt")),
                Raw::Modify(p("/w/note.txt")),
                Raw::Modify(p("/w/note.txt")),
                Raw::Remove(p("/w/note.txtzJmTMj.bck")),
            ],
            no_snapshot,
            u64::MAX,
        );
        assert_eq!(events, [Event::Change(p("/w/note.txt"))]);
        // Vim with `backup` set keeps the backup: it is a new file.
        let events = classify(
            &[
                Raw::Rename {
                    from: p("/w/a.rs"),
                    to: p("/w/a.rs~"),
                },
                Raw::Create(p("/w/a.rs")),
                Raw::Modify(p("/w/a.rs")),
            ],
            no_snapshot,
            u64::MAX,
        );
        assert_eq!(
            events,
            [Event::Change(p("/w/a.rs")), Event::Created(p("/w/a.rs~"))]
        );
    }

    #[test]
    fn an_unpaired_remove_and_create_pair_by_content() -> std::io::Result<()> {
        let dir = std::env::temp_dir().join(format!("fathomable-watch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir)?;
        let new = dir.join("new.md");
        let other = dir.join("other.md");
        fs::write(&new, "same content\n")?;
        fs::write(&other, "different\n")?;
        let old = dir.join("old.md");
        let seen = |path: &Path| (path == old).then(|| Fingerprint::from_bytes(b"same content\n"));
        let events = classify(
            &[
                Raw::Remove(old.clone()),
                Raw::Create(other.clone()),
                Raw::Create(new.clone()),
            ],
            seen,
            u64::MAX,
        );
        assert_eq!(
            events,
            [
                Event::Renamed {
                    from: old.clone(),
                    to: new.clone()
                },
                Event::Created(other.clone()),
            ]
        );
        // No snapshot, or a different one: a delete and a create.
        let events = classify(
            &[Raw::Remove(old.clone()), Raw::Create(new.clone())],
            |_| Some(Fingerprint::from_bytes(b"else\n")),
            u64::MAX,
        );
        assert_eq!(
            events,
            [Event::Removed(old.clone()), Event::Created(new.clone())]
        );
        // A created file over the viewer's limit is not read to find out.
        let events = classify(
            &[Raw::Remove(old.clone()), Raw::Create(new.clone())],
            seen,
            4,
        );
        assert_eq!(events, [Event::Removed(old), Event::Created(new)]);
        fs::remove_dir_all(&dir)
    }
}
