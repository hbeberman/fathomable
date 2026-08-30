// @okf-doc: /decisions/0028-live-workspace.md
//! Workspace watcher: turns `notify` events into tree, view, and thread
//! changes (ADR 0028).
//!
//! [`Watcher`] owns the recursive watch on the workspace root (falling
//! back to the open file's directory when the root cannot be watched, ADR
//! 0015) and the watch on the thread store's directory (ADR 0024). Raw
//! events go into a [`Batch`], which waits out the hint debounce and then
//! folds a burst into one [`Event`] per path: a plain [`Event::Change`],
//! a [`Event::Created`] or [`Event::Removed`] entry, or a
//! [`Event::Renamed`] pair. [`Event::Rescan`] preserves the platform's
//! warning that events were lost. Renames are paired from the platform's
//! own pairing first; an unpaired remove-then-create in one batch is
//! paired when the created file's size and content hash match what the
//! removed path last held.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use fathomable_core::annotations::short_hash;
use notify::event::{ModifyKind, RenameMode};
use notify::{EventKind, RecursiveMode, Watcher as _};
use tokio::sync::mpsc;

/// What a settled batch says happened to one path. Paths are absolute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
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
    pub fn path(&self) -> Option<&Path> {
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
pub struct Fingerprint {
    size: u64,
    hash: String,
}

impl Fingerprint {
    /// The fingerprint of `bytes`.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            size: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            hash: short_hash(bytes),
        }
    }

    /// The fingerprint of the file at `path`, when it can be read.
    fn read(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        fs::read(path).ok().as_deref().map(Self::from_bytes)
    }
}

/// One raw watcher notification, before debouncing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Raw {
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
}

/// Raw events from the platform watcher, in arrival order.
#[derive(Debug)]
pub struct Raws {
    rx: mpsc::UnboundedReceiver<Raw>,
}

impl Raws {
    /// The next event; `None` once the watcher is gone.
    pub async fn recv(&mut self) -> Option<Raw> {
        self.rx.recv().await
    }

    /// An event already waiting, without blocking.
    pub fn try_recv(&mut self) -> Option<Raw> {
        self.rx.try_recv().ok()
    }
}

/// Watches the workspace root recursively (ADR 0015). When that fails
/// (inotify limits), falls back to the directory of the visible document,
/// following it as it changes (a rename lands as a directory event, so the
/// file itself is never watched directly).
pub struct Watcher {
    inner: notify::RecommendedWatcher,
    target: Option<PathBuf>,
    recursive: bool,
    /// The thread store, watched through its directory (ADR 0024).
    store: Option<PathBuf>,
}

impl std::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watcher")
            .field("target", &self.target)
            .field("recursive", &self.recursive)
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

impl Watcher {
    /// Start a watcher; raw events arrive on the returned [`Raws`].
    ///
    /// # Errors
    ///
    /// Fails when the platform watcher cannot be created.
    pub fn new() -> anyhow::Result<(Self, Raws)> {
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
                target: None,
                recursive: false,
                store: None,
            },
            Raws { rx },
        ))
    }

    /// Watch everything under `root`; false when the watch cannot be set up.
    pub fn watch_root(&mut self, root: &Path) -> bool {
        match self.inner.watch(root, RecursiveMode::Recursive) {
            Ok(()) => {
                tracing::info!(root = %root.display(), "watching workspace");
                self.recursive = true;
            }
            Err(error) => {
                tracing::warn!(%error, root = %root.display(), "cannot watch workspace; watching the open file only");
                self.recursive = false;
            }
        }
        self.recursive
    }

    /// Follow the visible document when only its directory is watched.
    pub fn follow(&mut self, target: Option<&Path>) {
        if self.recursive || target == self.target.as_deref() {
            return;
        }
        if let Some(old) = self.target.as_ref().and_then(|p| p.parent()) {
            let _ = self.inner.unwatch(old);
        }
        if let Some(dir) = target.and_then(Path::parent)
            && let Err(error) = self.inner.watch(dir, RecursiveMode::NonRecursive)
        {
            tracing::warn!(%error, dir = %dir.display(), "cannot watch directory");
        }
        self.target = target.map(Path::to_path_buf);
    }

    /// Whether an event on `path` is one this watcher was asked for: in
    /// the fallback mode only the open file and the store count.
    pub fn is_target(&self, path: &Path) -> bool {
        self.recursive
            || self.target.as_deref() == Some(path)
            || self.store.as_deref() == Some(path)
    }

    /// Watch the directory holding the thread store, so appends by other
    /// writers are noticed (ADR 0024).
    pub fn watch_store(&mut self, store: Option<&Path>) {
        let Some(dir) = store.and_then(Path::parent) else {
            return;
        };
        match self.inner.watch(dir, RecursiveMode::NonRecursive) {
            Ok(()) => {
                tracing::info!(dir = %dir.display(), "watching the thread store");
                self.store = store.map(Path::to_path_buf);
            }
            Err(error) => {
                tracing::warn!(%error, dir = %dir.display(), "cannot watch the thread store");
            }
        }
    }
}

/// Whether a path under `.git` can move HEAD or the index. Object
/// writes, reflogs, and lock files churn constantly and change neither.
pub fn is_git_metadata(relative: &Path) -> bool {
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
pub struct Batch {
    raw: Vec<Raw>,
    flush_at: Option<Instant>,
}

impl Batch {
    /// Adds an event; the first one after a flush starts the quiet period.
    pub fn push(&mut self, raw: Raw, debounce: Duration) {
        self.raw.push(raw);
        self.flush_at
            .get_or_insert_with(|| Instant::now() + debounce);
    }

    /// Resolves once the quiet period ends; never while the batch is empty.
    pub async fn settled(&self) {
        match self.flush_at {
            Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
            None => std::future::pending().await,
        }
    }

    /// Fold the batch into one event per path. `last_seen` gives the
    /// fingerprint a removed path last had, for pairing an unpaired
    /// remove-then-create as a rename.
    pub fn take(&mut self, last_seen: impl Fn(&Path) -> Option<Fingerprint>) -> Vec<Event> {
        self.flush_at = None;
        let raw = std::mem::take(&mut self.raw);
        classify(&raw, last_seen)
    }
}

/// Fold raw events into one [`Event`] per path, in first-seen order.
///
/// Platform-paired renames win; a `RenameFrom` then a `RenameTo` in the
/// batch are paired in order (inotify reports them so and then repeats
/// them as a pair, which is folded away); what is left is created,
/// removed, or changed by the last thing that happened to the path, and
/// a removed path whose last-seen fingerprint matches a created file is
/// a rename after all.
fn classify(raw: &[Raw], last_seen: impl Fn(&Path) -> Option<Fingerprint>) -> Vec<Event> {
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
            .filter_map(|p| Fingerprint::read(&p).map(|f| (p, f)))
            .collect();
        for from in removed {
            let Some(seen) = last_seen(&from) else {
                continue;
            };
            if let Some(index) = created_prints.iter().position(|(_, f)| *f == seen) {
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

    use notify::EventKind;
    use notify::event::{
        AccessKind, AccessMode, CreateKind, Flag, ModifyKind, RemoveKind, RenameMode,
    };

    use super::{Event, Fingerprint, Raw, classify, is_git_metadata};

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
        );
        assert_eq!(events, [Event::Removed(old), Event::Created(new)]);
        fs::remove_dir_all(&dir)
    }
}
