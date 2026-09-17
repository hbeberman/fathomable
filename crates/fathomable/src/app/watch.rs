// @okf-doc: /decisions/0028-live-workspace.md
//! Workspace watcher: turns `notify` events into tree, view, and thread
//! changes (ADR 0028).
//!
//! [`Watcher`] owns one non-recursive watch per visible workspace
//! directory, explicit watches along the open file's path, and the watches
//! on thread, agent, and Git state (ADR 0015, 0024, 0070). Ignored build
//! trees therefore consume neither inotify watches nor event-loop work.
//! Raw events go into a [`Batch`], which waits out the hint debounce and
//! then folds a burst into one [`Event`] per path: a plain
//! [`Event::Change`], a [`Event::Created`] or [`Event::Removed`] entry, or
//! a [`Event::Renamed`] pair. [`Event::Rescan`] preserves the platform's
//! warning that events were lost. Renames are paired from the platform's
//! own pairing first; an unpaired remove-then-create in one batch is
//! paired when the created file's size and content hash match what the
//! removed path last held.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use fathomable_core::annotations::short_hash;
use fathomable_core::follow::Ignore;
use fathomable_core::workspace::{EntryKind, Workspace, is_rules_file};
use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;

use super::App;

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
        fs::read(path).ok().as_deref().map(Self::from_bytes)
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
    rx: mpsc::UnboundedReceiver<Raw>,
}

impl Raws {
    /// The next event; `None` once the watcher is gone.
    pub(crate) async fn recv(&mut self) -> Option<Raw> {
        self.rx.recv().await
    }

    /// An event already waiting, without blocking.
    pub(crate) fn try_recv(&mut self) -> Option<Raw> {
        self.rx.try_recv().ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchMode {
    NonRecursive,
    Recursive,
}

impl WatchMode {
    const fn notify(self) -> RecursiveMode {
        match self {
            Self::NonRecursive => RecursiveMode::NonRecursive,
            Self::Recursive => RecursiveMode::Recursive,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Surface {
    Root,
    Targets,
    State,
    Extras,
}

impl Surface {
    const fn bit(self) -> u8 {
        match self {
            Self::Root => 1 << 0,
            Self::Targets => 1 << 1,
            Self::State => 1 << 2,
            Self::Extras => 1 << 3,
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

    const fn surface_complete(self, surface: Surface) -> bool {
        self.0 & surface.bit() == 0
    }
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
    /// Ancestors needed to follow loaded files when they are ignored.
    target_dirs: HashSet<PathBuf>,
    /// Directories holding exact state files.
    state_dirs: HashSet<PathBuf>,
    /// The exact thread-state paths.
    state_files: HashSet<PathBuf>,
    /// Git metadata paths and whether each needs recursive coverage.
    extras: HashMap<PathBuf, WatchMode>,
    /// The watches currently installed in `notify`.
    watched: HashMap<PathBuf, WatchMode>,
    /// The active worktree root.
    root: Option<PathBuf>,
    coverage: Coverage,
}

impl std::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watcher")
            .field("targets", &self.targets)
            .field("visible_directories", &self.root_dirs.len())
            .field("target_directories", &self.target_dirs.len())
            .field("state_files", &self.state_files)
            .finish_non_exhaustive()
    }
}

impl Watcher {
    /// Start a watcher; raw events arrive on the returned [`Raws`].
    ///
    /// # Errors
    ///
    /// Fails when the platform watcher cannot be created.
    pub(crate) fn new() -> anyhow::Result<(Self, Raws)> {
        let (tx, rx) = mpsc::unbounded_channel::<Raw>();
        let watcher =
            notify::recommended_watcher(
                move |result: notify::Result<notify::Event>| match result {
                    // `notify` also reports opens (`Access`): our own reads of
                    // the document, `.gitignore`, and `.git` would otherwise
                    // feed back as changes, forever.
                    Ok(event) => {
                        tracing::debug!(kind = ?event.kind, paths = ?event.paths, "watcher event");
                        for raw in Raw::from_notify(event) {
                            let _ = tx.send(raw);
                        }
                    }
                    Err(error) => tracing::warn!(%error, "file watcher error"),
                },
            )
            .context("cannot create file watcher")?;
        Ok((
            Self {
                inner: watcher,
                targets: HashSet::new(),
                root_dirs: HashSet::new(),
                target_dirs: HashSet::new(),
                state_dirs: HashSet::new(),
                state_files: HashSet::new(),
                extras: HashMap::new(),
                watched: HashMap::new(),
                root: None,
                coverage: Coverage::default(),
            },
            Raws { rx },
        ))
    }

    /// Reconcile non-recursive watches with the visible directory tree.
    ///
    /// Each directory is watched before it is listed, so a child created
    /// during the walk is still reported by its parent. Returns whether
    /// every visible directory is covered.
    pub(crate) fn watch_root(&mut self, workspace: &mut Workspace, ignore: &Ignore) -> bool {
        let root = workspace.root().to_path_buf();
        let changed_root = self.root.as_deref() != Some(root.as_path());
        self.root = Some(root.clone());
        let old = std::mem::take(&mut self.root_dirs);
        let mut pending = vec![PathBuf::new()];
        let mut complete = true;

        while let Some(relative) = pending.pop() {
            if !relative.as_os_str().is_empty() && ignore.is_tree_ignored(&relative) {
                continue;
            }
            let absolute = if relative.as_os_str().is_empty() {
                root.clone()
            } else {
                root.join(&relative)
            };
            self.root_dirs.insert(absolute.clone());
            if let Err(error) = self.reconcile(&absolute) {
                tracing::warn!(%error, dir = %absolute.display(), "cannot watch visible directory");
                self.root_dirs.remove(&absolute);
                complete = false;
            }
            let entries = match workspace.list_dir(&relative) {
                Ok(entries) => entries,
                Err(error) => {
                    if relative.as_os_str().is_empty() {
                        tracing::warn!(%error, root = %root.display(), "cannot list workspace while watching");
                        complete = false;
                    } else {
                        tracing::debug!(%error, dir = %absolute.display(), "directory gone while watching");
                    }
                    continue;
                }
            };
            let mut dirs: Vec<PathBuf> = entries
                .into_iter()
                .filter(|entry| entry.is_dir() && !entry.is_symlink())
                .map(|entry| relative.join(entry.name()))
                .filter(|path| !ignore.is_tree_ignored(path))
                .collect();
            pending.extend(dirs.drain(..).rev());
        }

        for stale in old {
            if !self.root_dirs.contains(&stale)
                && let Err(error) = self.reconcile(&stale)
            {
                tracing::debug!(%error, dir = %stale.display(), "cannot remove stale directory watch");
            }
        }
        if changed_root {
            tracing::info!(
                root = %root.display(),
                directories = self.root_dirs.len(),
                "watching visible workspace"
            );
        }
        self.coverage.set(Surface::Root, complete);
        complete
    }

    /// Add a newly-created visible directory before its debounce settles.
    ///
    /// Files already present when their directory gets its watch are
    /// returned as synthetic creates. This closes the mkdir-then-populate
    /// race without watching ignored trees.
    pub(crate) fn watch_created(
        &mut self,
        workspace: &mut Workspace,
        ignore: &Ignore,
        created: &Path,
    ) -> (Vec<Raw>, bool) {
        let Some(root) = self.root.clone() else {
            return (Vec::new(), true);
        };
        let Ok(relative) = created.strip_prefix(&root).map(Path::to_path_buf) else {
            return (Vec::new(), true);
        };
        if !created
            .symlink_metadata()
            .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
        {
            return (Vec::new(), true);
        }
        if ignore.is_tree_ignored(&relative)
            || workspace.is_ignored(&relative, fathomable_core::workspace::EntryKind::Dir)
        {
            return (Vec::new(), true);
        }

        let mut pending = vec![relative];
        let mut discovered = Vec::new();
        let mut complete = true;
        while let Some(relative) = pending.pop() {
            if ignore.is_tree_ignored(&relative) {
                continue;
            }
            let absolute = root.join(&relative);
            let newly_watched = self.root_dirs.insert(absolute.clone());
            if let Err(error) = self.reconcile(&absolute) {
                tracing::warn!(%error, dir = %absolute.display(), "cannot watch new directory");
                self.root_dirs.remove(&absolute);
                complete = false;
            }
            let entries = match workspace.list_dir(&relative) {
                Ok(entries) => entries,
                Err(error) => {
                    tracing::debug!(%error, dir = %absolute.display(), "new directory gone while watching");
                    continue;
                }
            };
            let mut dirs = Vec::new();
            for entry in entries {
                let child = relative.join(entry.name());
                if entry.is_dir() && !entry.is_symlink() {
                    if !ignore.is_tree_ignored(&child) {
                        dirs.push(child);
                    }
                } else if !ignore.is_ignored(&child) && newly_watched {
                    discovered.push(Raw::Create(root.join(child)));
                }
            }
            pending.extend(dirs.into_iter().rev());
        }
        if !complete {
            self.coverage.set(Surface::Root, false);
        }
        (discovered, complete)
    }

    /// Follow every loaded document, including through ignored trees.
    ///
    /// A rename is reported by the watched parent, so every directory from
    /// the root to the file's parent is covered rather than the file itself.
    pub(crate) fn follow<'a>(&mut self, targets: impl IntoIterator<Item = &'a Path>) -> bool {
        let targets: HashSet<PathBuf> = targets.into_iter().map(Path::to_path_buf).collect();
        if targets == self.targets {
            return self.coverage.surface_complete(Surface::Targets);
        }
        let old = std::mem::take(&mut self.target_dirs);
        let mut complete = true;
        for parent in targets.iter().filter_map(|target| target.parent()) {
            if let Some(root) = self.root.as_deref()
                && parent.starts_with(root)
            {
                let mut ancestors: Vec<PathBuf> = parent
                    .ancestors()
                    .take_while(|dir| dir.starts_with(root))
                    .map(Path::to_path_buf)
                    .collect();
                ancestors.reverse();
                self.target_dirs.extend(ancestors);
            } else {
                self.target_dirs.insert(parent.to_path_buf());
            }
        }
        let wanted: Vec<PathBuf> = self.target_dirs.iter().cloned().collect();
        for dir in wanted {
            if let Err(error) = self.reconcile(&dir) {
                tracing::warn!(%error, dir = %dir.display(), "cannot watch loaded file directory");
                self.target_dirs.remove(&dir);
                complete = false;
            }
        }
        for stale in old {
            if !self.target_dirs.contains(&stale) {
                let _ = self.reconcile(&stale);
            }
        }
        self.targets = targets;
        self.coverage.set(Surface::Targets, complete);
        complete
    }

    /// Whether every requested live surface has complete coverage.
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
            || self.targets.contains(path)
            || self.extras.keys().any(|dir| path.starts_with(dir))
            || path.parent().is_some_and(|parent| {
                self.root_dirs.contains(parent) || self.target_dirs.contains(parent)
            })
    }

    /// Watch the git paths of the other worktrees (ADR 0070), each on
    /// its own and not recursively, except the refs, which are few and
    /// nested: a `HEAD` or a branch moving there changes the reach.
    pub(crate) fn watch_worktrees(&mut self, paths: &[PathBuf]) -> bool {
        let old = std::mem::take(&mut self.extras);
        self.extras = paths
            .iter()
            .map(|path| {
                let mode = if path.file_name().is_some_and(|name| name == "refs") {
                    WatchMode::Recursive
                } else {
                    WatchMode::NonRecursive
                };
                (path.clone(), mode)
            })
            .collect();
        let mut complete = true;
        let wanted: Vec<PathBuf> = self.extras.keys().cloned().collect();
        for path in wanted {
            if let Err(error) = self.reconcile(&path) {
                tracing::debug!(%error, dir = %path.display(), "cannot watch worktree path");
                self.extras.remove(&path);
                complete = false;
            }
        }
        for stale in old.keys() {
            if !self.extras.contains_key(stale) {
                let _ = self.reconcile(stale);
            }
        }
        self.coverage.set(Surface::Extras, complete);
        complete
    }

    /// Watch the directories holding exact state `files`.
    pub(crate) fn watch_state<'a>(&mut self, files: impl IntoIterator<Item = &'a Path>) -> bool {
        let old = std::mem::take(&mut self.state_dirs);
        self.state_files = files.into_iter().map(Path::to_path_buf).collect();
        self.state_dirs = self
            .state_files
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect();
        let mut complete = true;
        let wanted: Vec<PathBuf> = self.state_dirs.iter().cloned().collect();
        for dir in wanted {
            if let Err(error) = self.reconcile(&dir) {
                tracing::warn!(%error, dir = %dir.display(), "cannot watch workspace state");
                self.state_dirs.remove(&dir);
                complete = false;
            }
        }
        for stale in old {
            if !self.state_dirs.contains(&stale) {
                let _ = self.reconcile(&stale);
            }
        }
        self.coverage.set(Surface::State, complete);
        complete
    }

    /// Whether structural `events` can have changed the visible directory
    /// watch set.
    pub(crate) fn needs_root_sync(&self, events: &[Event]) -> bool {
        events.iter().any(|event| match event {
            Event::Created(path) => path.is_dir(),
            Event::Removed(path) => self.root_dirs.iter().any(|dir| dir.starts_with(path)),
            Event::Renamed { from, to } => {
                to.is_dir() || self.root_dirs.iter().any(|dir| dir.starts_with(from))
            }
            Event::Change(_) | Event::Rescan => false,
        })
    }

    fn desired_mode(&self, path: &Path) -> Option<WatchMode> {
        self.extras.get(path).copied().or_else(|| {
            (self.root_dirs.contains(path)
                || self.target_dirs.contains(path)
                || self.state_dirs.contains(path))
            .then_some(WatchMode::NonRecursive)
        })
    }

    fn reconcile(&mut self, path: &Path) -> notify::Result<()> {
        let wanted = self.desired_mode(path);
        let have = self.watched.get(path).copied();
        if wanted == have {
            return Ok(());
        }
        if have.is_some() {
            let _ = self.inner.unwatch(path);
            self.watched.remove(path);
        }
        let Some(mode) = wanted else {
            return Ok(());
        };
        self.inner.watch(path, mode.notify())?;
        self.watched.insert(path.to_path_buf(), mode);
        Ok(())
    }
}

impl App {
    /// Install or reconcile watches for the active visible workspace.
    pub(crate) fn sync_workspace_watches(&mut self, watcher: &mut Watcher) -> bool {
        watcher.watch_root(&mut self.workspace, &self.ignore)
    }

    /// Cover a newly-created visible directory before its event settles.
    pub(crate) fn watch_created(&mut self, watcher: &mut Watcher, path: &Path) -> (Vec<Raw>, bool) {
        watcher.watch_created(&mut self.workspace, &self.ignore, path)
    }

    /// Absolute paths of every document retained by this viewer.
    pub(crate) fn loaded_abs_paths(&self) -> Vec<PathBuf> {
        let root = self.workspace.root();
        self.docs
            .iter()
            .map(|doc| root.join(&doc.relative))
            .collect()
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
#[derive(Debug, Default)]
pub(crate) struct Batch {
    raw: Vec<Raw>,
    flush_at: Option<Instant>,
}

impl Batch {
    /// Adds an event and restarts the quiet period.
    pub(crate) fn push(&mut self, raw: Raw, debounce: Duration) {
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
    use std::path::{Path, PathBuf};

    use fathomable_core::follow::Ignore;
    use fathomable_core::workspace::Workspace;
    use fathomable_testing::{TempDir, git};
    use notify::EventKind;
    use notify::event::{
        AccessKind, AccessMode, CreateKind, Flag, ModifyKind, RemoveKind, RenameMode,
    };

    use super::{Event, Fingerprint, Raw, Watcher, classify, is_git_metadata};

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn no_snapshot(_: &Path) -> Option<Fingerprint> {
        None
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

    #[test]
    fn workspace_watches_cover_visible_directories_not_build_trees() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-visible")?;
        git::init(&dir.0)?;
        fs::write(dir.0.join(".gitignore"), "target/\n.tmp/\n")?;
        fs::create_dir_all(dir.0.join("src/nested"))?;
        fs::create_dir_all(dir.0.join("target/deep/cache"))?;
        fs::create_dir_all(dir.0.join(".tmp/worktrees/generated"))?;
        fs::create_dir_all(dir.0.join("build/deep"))?;
        let mut workspace = Workspace::discover(&dir.0)?;
        let ignore = Ignore::new(&["build/**".to_owned()])?;
        let (mut watcher, _) = Watcher::new()?;

        assert!(watcher.watch_root(&mut workspace, &ignore));
        assert_eq!(
            watcher.root_dirs,
            [dir.0.clone(), dir.0.join("src"), dir.0.join("src/nested"),]
                .into_iter()
                .collect()
        );
        Ok(())
    }

    #[test]
    fn a_new_visible_subtree_is_watched_before_its_files_are_reported() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-created")?;
        git::init(&dir.0)?;
        fs::create_dir_all(dir.0.join("src"))?;
        let mut workspace = Workspace::discover(&dir.0)?;
        let ignore = Ignore::default();
        let (mut watcher, _) = Watcher::new()?;
        assert!(watcher.watch_root(&mut workspace, &ignore));

        fs::create_dir_all(dir.0.join("src/new/deep"))?;
        fs::write(dir.0.join("src/new/deep/file.rs"), "fn main() {}\n")?;
        let (created, complete) =
            watcher.watch_created(&mut workspace, &ignore, &dir.0.join("src/new"));
        assert!(complete);
        assert!(watcher.root_dirs.contains(&dir.0.join("src/new")));
        assert!(watcher.root_dirs.contains(&dir.0.join("src/new/deep")));
        assert_eq!(created, [Raw::Create(dir.0.join("src/new/deep/file.rs"))]);
        Ok(())
    }

    #[test]
    fn loaded_ignored_files_keep_narrow_ancestor_watches() -> anyhow::Result<()> {
        let dir = TempDir::new("watch-open-ignored")?;
        git::init(&dir.0)?;
        fs::write(dir.0.join(".gitignore"), "target/\n")?;
        fs::create_dir_all(dir.0.join("target/one"))?;
        fs::create_dir_all(dir.0.join("target/two"))?;
        let first = dir.0.join("target/one/output.txt");
        let second = dir.0.join("target/two/output.txt");
        fs::write(&first, "one\n")?;
        fs::write(&second, "two\n")?;
        let mut workspace = Workspace::discover(&dir.0)?;
        let (mut watcher, _) = Watcher::new()?;
        assert!(watcher.watch_root(&mut workspace, &Ignore::default()));

        assert!(watcher.follow([first.as_path(), second.as_path()]));
        assert!(watcher.target_dirs.contains(&dir.0.join("target")));
        assert!(watcher.target_dirs.contains(&dir.0.join("target/one")));
        assert!(watcher.target_dirs.contains(&dir.0.join("target/two")));
        assert!(watcher.accepts(&Raw::Modify(first)));
        assert!(watcher.accepts(&Raw::Modify(second)));
        Ok(())
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
