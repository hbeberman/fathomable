// @okf-doc: /decisions/0012-workspace-mode.md
//! The workspace app: open documents, sidebar, popups, and the state every
//! concept module hangs off.
//!
//! [`App`] is plain state so the viewer's behaviour is tested without a
//! terminal. The modules are grouped by concept (ADR 0048): `threads`
//! holds the thread cursor, the panes, the list, and the store operations;
//! `draw` renders; `input` binds and dispatches keys and the mouse; `jump`
//! is auto-jump; `agents` wakes subscribers; `view`, `sidebar`, `watch`,
//! `socket`, `commands`, and `clipboard` are what their names say; and
//! [`run`] owns the terminal, the file watcher, and the viewer socket.

pub(crate) mod agents;
mod clipboard;
mod commands;
mod draw;
pub(crate) mod input;
mod jump;
mod jumplist;
pub(crate) mod run;
mod sidebar;
mod socket;
pub(crate) mod threads;
mod view;
mod watch;

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::threads::list::ThreadList;
use fathomable_core::annotations::{self, Reach, Store, ThreadId};
use fathomable_core::config::{
    AgentsConfig, JumpConfig, MarkdownConfig, RailConfig, ThreadsConfig, ViewerConfig, WatchConfig,
};
use fathomable_core::content::Policy;
use fathomable_core::diff::Diff;
use fathomable_core::editor::Cell;
use fathomable_core::follow::{Change, Ignore, Queue, Target};
use fathomable_core::highlight::{Highlighter, language_hint};
use fathomable_core::picker::{Match, Picker};
use fathomable_core::seen;
use fathomable_core::session::{FollowState, Record, Request, Response};
use fathomable_core::status::Status;
use fathomable_core::tree::Tree;
use fathomable_core::workspace::{EntryKind, Filter, Workspace};
use fathomable_core::{Document, XdgDirs};
use input::bindings::Chord;

pub use threads::cursor::ThreadCursor;
pub use threads::{Compose, Mark};
use view::{HunkStep, Syntax, View};
use watch::{Fingerprint, is_git_metadata};

/// How long to wait after a change notification before re-reading, so an
/// editor's write-then-rename lands as one reload.
/// Toasts visible at once.
pub const MAX_TOASTS: usize = 3;

/// A transient one-line notice about a change (ADR 0015).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    text: String,
    until: Instant,
}

impl Toast {
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Narrowest the tree can be dragged.
const SIDEBAR_MIN_WIDTH: usize = 8;

/// Fewest text columns a drag leaves the view.
const TEXT_MIN_WIDTH: usize = 20;

/// Most rows the comment box grows to on its own before it scrolls.
const COMPOSE_MAX_ROWS: usize = 8;
/// Rule, header, and one line of text.
const COMPOSE_MIN_ROWS: usize = 3;

/// Rows kept visible above and below the sidebar cursor.
const SIDEBAR_SCROLLOFF: usize = 2;

/// Which pane receives keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    View,
    Sidebar,
    /// The review list (ADR 0025, ADR 0049).
    Review,
    /// The rail's threads pane (ADR 0027, ADR 0049).
    ThreadsPane,
}

/// A pane border the mouse is dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Border {
    /// The rule between the tree and the text.
    Sidebar,
    /// The rule along the top of the comment box (ADR 0018).
    Compose,
    /// The rule along the top of the threads pane (ADR 0027).
    ThreadsPane,
}

/// What the file picker lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    /// Workspace files, ignore-filtered.
    Files,
    /// Workspace files including ignored ones.
    AllFiles,
    /// Documents opened this session, most recent first.
    Recent,
    /// Subscribed agents to wake with `Space w` (ADR 0040).
    Wake,
}

/// The open picker popup.
#[derive(Debug)]
pub struct PickerState {
    kind: PickerKind,
    picker: Picker,
    input: String,
    matches: Vec<Match>,
    selected: usize,
}

impl PickerState {
    fn new(kind: PickerKind, items: Vec<String>) -> Self {
        let mut picker = Picker::new(items);
        let matches = picker.query("");
        Self {
            kind,
            picker,
            input: String::new(),
            matches,
            selected: 0,
        }
    }

    pub fn kind(&self) -> PickerKind {
        self.kind
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn item(&self, m: &Match) -> &str {
        &self.picker.items()[m.index()]
    }

    /// Number of candidates before filtering.
    pub fn total(&self) -> usize {
        self.picker.items().len()
    }

    fn requery(&mut self) {
        self.matches = self.picker.query(&self.input);
        self.selected = 0;
    }
}

/// A popup layered over the panes.
#[derive(Debug)]
pub enum Popup {
    /// Every key binding.
    Help,
    /// A file, recent-document, or thread picker.
    Picker(PickerState),
    /// The comment box (ADR 0013).
    Compose(Compose),
    /// The `:status` overlay (ADR 0021).
    Status,
}

#[derive(Debug)]
struct Doc {
    document: Document,
    relative: PathBuf,
    view: View,
    marks: Vec<Mark>,
    /// Whether the text has changed or been read since it was last
    /// snapshotted as seen.
    seen_dirty: bool,
    /// Set while the file is gone from disk (ADR 0028); the last content
    /// stays loaded until it returns.
    deleted: Option<Deleted>,
}

/// How a deleted open file is shown (ADR 0028).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deleted {
    /// Deleted under the reader: the last content under a banner row.
    Banner,
    /// Shown again while still gone: the file-info pane.
    Info,
}

/// All application state.
#[derive(Debug)]
pub struct App {
    workspace: Workspace,
    docs: Vec<Doc>,
    current: Option<usize>,
    /// Documents by index, most recently shown first (`Space o`).
    recent: Vec<usize>,
    jumplist: jumplist::Jumplist,
    /// Where a search started, recorded on the jumplist when it lands
    /// somewhere else (ADR 0049).
    search_origin: Option<jumplist::Position>,
    welcome: View,
    tree: Option<Tree>,
    /// The rail's panes, scope, split, and sizes (ADR 0049).
    rail: threads::pane::Rail,
    /// Whether stubs are drawn, and for resolved threads (ADR 0049).
    stubs: threads::stubs::StubState,
    /// The threads expanded in place this session (ADR 0049).
    expanded: HashSet<ThreadId>,
    /// A `c` cycle in progress: the thread it started on and the threads
    /// it walks (ADR 0049).
    cycle: Option<(ThreadId, Vec<ThreadId>)>,
    /// What the review shows, shared by the list and the threads pane.
    review: threads::list::ReviewState,
    sidebar_scroll: usize,
    /// Tree width once dragged; the default follows the terminal.
    sidebar_cols: Option<usize>,
    /// The thread and message the thread surfaces show; authoritative
    /// while the pane or the list is open, or the text cursor rests
    /// where `thread_cursor_anchor` says it was set (ADR 0046).
    thread_cursor: ThreadCursor,
    /// `(document, row)` of the text cursor when the thread cursor was
    /// last set.
    thread_cursor_anchor: Option<(Option<usize>, usize)>,
    /// The thread list shown in place of the document (ADR 0025).
    list: ThreadList,
    /// File-threads pane height once dragged; the default follows its
    /// Comment box height once dragged; the default follows its text.
    compose_rows: Option<usize>,
    /// The border a mouse drag is moving.
    drag: Option<Border>,
    focus: Focus,
    popup: Option<Popup>,
    file_index: Option<Vec<String>>,
    all_index: Option<Vec<String>>,
    message: Option<String>,
    /// The keys typed so far of a longer binding (ADR 0045).
    prefix: Vec<Chord>,
    /// The thread a first `d` armed for deletion (ADR 0034).
    pending_delete: Option<annotations::ThreadId>,
    width: usize,
    height: usize,
    viewer_id: String,
    store: Option<Store>,
    /// Which threads the current `HEAD` shows (ADR 0024).
    reach: Reach,
    /// Files an agent said it is working on (ADR 0014 `follow`).
    followed: Vec<PathBuf>,
    record: Record,
    dirs: XdgDirs,
    jump: JumpConfig,
    /// Code highlighting shared by every view (ADR 0016).
    highlighter: Arc<Highlighter>,
    /// Which files render as Markdown (ADR 0016).
    markdown: MarkdownConfig,
    /// How files are read (ADR 0026).
    viewer: ViewerConfig,
    /// Subscriptions and the wake command (ADR 0040).
    agents: AgentsConfig,
    /// Who watches which thread, refreshed with the store (ADR 0040).
    watchers: Vec<(ThreadId, String)>,
    /// The config file the over-limit notice names (ADR 0026).
    config_path: PathBuf,
    auto: bool,
    ignore: Ignore,
    queue: Queue,
    toasts: Vec<Toast>,
    seen: Option<seen::Store>,
    /// When the queue last changed, for the auto-jump debounce.
    last_change: Option<Instant>,
    /// Whether the recursive workspace watch is in place.
    watching_root: bool,
    /// Every uncommitted path (ADR 0017), refreshed on git and file events.
    status: Status,
}

impl App {
    /// Start with no document open, for a terminal of `width` by `height`.
    ///
    /// Threads on files edited while Fathomable was closed are followed
    /// through their last-seen snapshots before anything opens (ADR 0020).
    pub fn new(workspace: Workspace, width: usize, height: usize, options: Options) -> Self {
        let Options {
            record,
            dirs,
            store,
            jump,
            watch,
            seen,
            highlighter,
            markdown,
            viewer,
            rail,
            threads,
            agents,
            config_path,
        } = options;
        let ignore = match Ignore::new(&watch.ignore) {
            Ok(ignore) => ignore,
            Err(error) => {
                tracing::warn!(%error, "ignoring watch.ignore");
                Ignore::default()
            }
        };
        let mut app = Self {
            workspace,
            docs: Vec::new(),
            current: None,
            recent: Vec::new(),
            jumplist: jumplist::Jumplist::default(),
            search_origin: None,
            welcome: View::new(String::new(), 1, 1),
            tree: None,
            rail: threads::pane::Rail::new(rail),
            stubs: threads::stubs::StubState::from_config(&threads),
            expanded: HashSet::new(),
            cycle: None,
            review: threads::list::ReviewState::default(),
            sidebar_scroll: 0,
            sidebar_cols: None,
            thread_cursor: ThreadCursor::default(),
            thread_cursor_anchor: None,
            list: ThreadList::default(),
            compose_rows: None,
            drag: None,
            focus: Focus::View,
            popup: None,
            file_index: None,
            all_index: None,
            message: None,
            prefix: Vec::new(),
            pending_delete: None,
            width,
            height,
            viewer_id: record.id().to_string(),
            store,
            reach: Reach::everything(),
            followed: Vec::new(),
            record,
            dirs,
            auto: jump.auto,
            jump,
            highlighter,
            markdown,
            viewer,
            agents,
            watchers: Vec::new(),
            config_path,
            ignore,
            queue: Queue::default(),
            toasts: Vec::new(),
            seen,
            last_change: None,
            watching_root: false,
            status: Status::default(),
        };
        app.relayout();
        app.refresh_status();
        // Snapshots first: a thread edited offline must locate before the
        // scope refresh can keep it across a rewrite (ADR 0035).
        app.reanchor_from_snapshots();
        app.refresh_reach();
        app
    }

    /// Recompute which threads `HEAD` shows (ADR 0024): one history walk
    /// for the commits the store mentions. Open threads a rewrite
    /// stranded follow `HEAD` first (ADR 0035). Marks are refreshed when
    /// the answer changed.
    pub(super) fn refresh_reach(&mut self) {
        let scope = match self.store.as_mut() {
            Some(store) => match self.workspace.reachable(store.commits()) {
                Some(mut reachable) => {
                    if crate::app::threads::reach::follow_head(store, &self.workspace, &reachable)
                        > 0
                    {
                        reachable.extend(self.workspace.head_commit());
                    }
                    Reach::reachable(reachable)
                }
                None => Reach::everything(),
            },
            None => Reach::everything(),
        };
        if scope != self.reach {
            tracing::info!(head = ?self.workspace.head_commit(), "thread reach changed");
            self.reach = scope;
            for index in 0..self.docs.len() {
                self.refresh_marks(index);
            }
        }
    }

    /// Re-read the thread store after another writer appended to it: a
    /// second viewer, or a headless `--mcp` reply (ADR 0024). Marks and the
    /// open thread panel follow.
    pub fn reload_store(&mut self) {
        let Some(path) = self.store.as_ref().map(|store| store.path().to_path_buf()) else {
            return;
        };
        match Store::open(&path) {
            Ok(store) => {
                let changed = self
                    .store
                    .as_ref()
                    .is_none_or(|old| old.threads() != store.threads());
                if !changed {
                    return;
                }
                tracing::info!(threads = store.threads().len(), "thread store reloaded");
                let before = self
                    .store
                    .as_ref()
                    .map(Self::waiting_ids)
                    .unwrap_or_default();
                self.store = Some(store);
                self.toast_waiting(&before);
                self.refresh_watchers();
                self.refresh_reach();
                for index in 0..self.docs.len() {
                    self.refresh_marks(index);
                }
            }
            Err(error) => tracing::warn!(%error, "cannot reload the thread store"),
        }
    }

    /// Where the thread store lives, for the watcher.
    pub fn store_path(&self) -> Option<&Path> {
        self.store.as_ref().map(Store::path)
    }

    /// `:name`: rename this viewer for agents; empty clears the name.
    pub fn set_name(&mut self, name: Option<&str>) {
        self.record = self.record.clone().with_name(name.map(str::to_owned));
        match self.record.write(&self.dirs) {
            Ok(()) => self.notice(match self.record.name() {
                Some(name) => format!("viewer named {name}"),
                None => "viewer name cleared".to_owned(),
            }),
            Err(error) => self.notice(format!("cannot save the viewer name: {error}")),
        }
    }

    /// How a root-relative `path` should be coloured and first displayed.
    /// The code highlighter shared by every view and the thread pane.
    pub fn highlighter(&self) -> &Highlighter {
        &self.highlighter
    }

    fn syntax_for(&self, path: &Path) -> Syntax {
        Syntax {
            highlighter: Arc::clone(&self.highlighter),
            hint: language_hint(path),
            markdown: self.markdown.matches(path),
        }
    }

    /// Whether the whole workspace is watched, or only the visible
    /// document's directory as a fallback.
    pub fn set_watching_root(&mut self, watching: bool) {
        self.watching_root = watching;
    }

    // ----- follow mode (ADR 0015) -----

    /// Whether auto-jump is on.
    pub fn auto_jump(&self) -> bool {
        self.auto
    }

    /// Changed files, newest first.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// Live toasts, oldest first.
    pub fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// Whether a queued change sits at or under the root-relative `path`.
    pub fn has_change_under(&self, path: &Path) -> bool {
        self.queue.iter().any(|c| c.path.starts_with(path))
    }

    /// Toggle auto-jump (`Space j a`, `:auto`).
    pub fn toggle_auto_jump(&mut self) {
        self.auto = !self.auto;
        self.notice(if self.auto {
            "auto-jump on"
        } else {
            "auto-jump off"
        });
    }

    /// Drop every queued change (`Space j c`).
    pub fn clear_queue(&mut self) {
        self.queue.clear();
        self.last_change = None;
    }

    /// Set auto-jump (`:auto on` / `:auto off`).
    pub fn set_auto_jump(&mut self, on: bool) {
        self.auto = on;
        self.notice(if on { "auto-jump on" } else { "auto-jump off" });
    }

    /// `Space j j`: open the newest change.
    pub fn jump_newest(&mut self) {
        match self.queue.newest().cloned() {
            Some(change) => self.jump_to(&change),
            None => self.notice("no changes"),
        }
    }

    /// `]f`: the next older change after the current file, wrapping.
    pub fn jump_next(&mut self) {
        let current = self.current.map(|i| self.docs[i].relative.clone());
        match self.queue.after(current.as_deref()).cloned() {
            Some(change) => self.jump_to(&change),
            None => self.notice("no changes"),
        }
    }

    /// `[f`: the next newer change before the current file, wrapping.
    pub fn jump_prev(&mut self) {
        let current = self.current.map(|i| self.docs[i].relative.clone());
        match self.queue.before(current.as_deref()).cloned() {
            Some(change) => self.jump_to(&change),
            None => self.notice("no changes"),
        }
    }

    fn jump_to(&mut self, change: &Change) {
        self.close_popup();
        self.focus = Focus::View;
        self.record_jump_from_here();
        self.open(&change.path);
        if self.current_path() != change.path {
            return;
        }
        let view = self.view_mut();
        view.escape();
        match change.target {
            Target::Line(line) => view.goto_source_line(line),
            Target::Range(start, end) => {
                view.goto_source_line(start);
                if end > start {
                    view.select_lines();
                    view.goto_source_line(end);
                }
            }
        }
        self.queue.remove(&change.path);
        tracing::info!(path = %change.path.display(), "jumped to change");
    }

    /// Files the watcher reported, absolute: a change for each that
    /// exists, a removal for each that does not.
    #[cfg(test)]
    pub fn on_changes(&mut self, paths: Vec<PathBuf>) {
        let events = paths
            .into_iter()
            .map(|path| {
                if path.exists() {
                    watch::Event::Change(path)
                } else {
                    watch::Event::Removed(path)
                }
            })
            .collect();
        self.on_events(events);
    }

    /// What one settled watcher batch did (ADR 0028). Loaded documents
    /// reload, follow their rename, or keep their content under a
    /// `deleted` banner; changes that pass the source and ignore rules
    /// join the queue; the directories whose listings changed are
    /// re-read in the tree. A platform event-loss notice reconciles the
    /// whole remembered workspace from disk.
    pub fn on_events(&mut self, events: Vec<watch::Event>) {
        if events
            .iter()
            .any(|event| matches!(event, watch::Event::Rescan))
        {
            self.rescan_workspace();
            return;
        }
        // The store lives outside the root; another writer's append lands
        // here through the store watch (ADR 0024).
        if events.iter().any(|e| e.path() == self.store_path()) {
            self.reload_store();
        }
        let root = self.workspace.root().to_path_buf();
        let banner_before = self.banner().is_some();
        let mut git_changed = false;
        let mut touched = false;
        // Root-relative directories whose listing changed.
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();
        for event in events {
            let event = match event {
                // A move across the root's edge is a plain arrival or
                // departure on this side of it.
                watch::Event::Renamed { from, to } => {
                    match (from.starts_with(&root), to.starts_with(&root)) {
                        (true, true) => watch::Event::Renamed { from, to },
                        (true, false) => watch::Event::Removed(from),
                        (false, true) => watch::Event::Created(to),
                        (false, false) => continue,
                    }
                }
                other => other,
            };
            let Some(path) = event.path() else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(&root).map(Path::to_path_buf) else {
                continue;
            };
            if relative.starts_with(".git") {
                git_changed |= is_git_metadata(&relative);
                continue;
            }
            touched = true;
            match event {
                watch::Event::Change(absolute) | watch::Event::Created(absolute) => {
                    if !seen_paths.insert(relative.clone()) {
                        continue;
                    }
                    let listed = self
                        .tree
                        .as_ref()
                        .is_some_and(|tree| tree.contains(&relative));
                    if !listed {
                        self.note_dir(&relative, &mut dirs);
                    }
                    self.on_change(&relative, &absolute);
                }
                watch::Event::Removed(_) => {
                    self.note_dir(&relative, &mut dirs);
                    self.on_removed(&relative);
                }
                watch::Event::Renamed { from, .. } => {
                    let Ok(from) = from.strip_prefix(&root).map(Path::to_path_buf) else {
                        continue;
                    };
                    self.note_dir(&from, &mut dirs);
                    self.note_dir(&relative, &mut dirs);
                    self.on_renamed(&from, &relative);
                }
                watch::Event::Rescan => {}
            }
        }
        if git_changed {
            tracing::info!("git metadata changed; refreshing HEAD bases");
            for index in 0..self.docs.len() {
                self.refresh_base(index);
            }
            self.refresh_reach();
        }
        if git_changed || touched {
            self.refresh_status();
        }
        // The banner row takes a text row, so the view re-fits when it
        // comes or goes.
        if banner_before != self.banner().is_some() {
            self.relayout();
        }
        if !dirs.is_empty() {
            // A new file is one `Space f` away (ADR 0028).
            self.file_index = None;
            self.all_index = None;
            for dir in dirs {
                self.with_tree_result(|tree, workspace| {
                    tree.refresh_dir(workspace, &dir).map(|_| None)
                });
            }
        }
    }

    /// Reconcile state after the platform reports that watcher events were
    /// lost. The tree, loaded documents, repository bases, status, and
    /// annotation store may all have changed during the gap.
    fn rescan_workspace(&mut self) {
        tracing::warn!("file watcher lost events; rescanning workspace");
        self.reload_store();
        for index in 0..self.docs.len() {
            self.refresh_base(index);
        }
        self.refresh_reach();

        let root = self.workspace.root().to_path_buf();
        let loaded: Vec<PathBuf> = self.docs.iter().map(|doc| doc.relative.clone()).collect();
        for relative in loaded {
            let absolute = root.join(&relative);
            if absolute.is_file() {
                self.on_change(&relative, &absolute);
            } else {
                self.on_removed(&relative);
            }
        }
        self.refresh_status();
        self.file_index = None;
        self.all_index = None;
        self.with_tree_result(|tree, workspace| tree.refresh(workspace).map(|()| None));
    }

    /// Note that the listing holding root-relative `path` changed, unless
    /// the tree would hide the path anyway (ADR 0028): build output
    /// churning under `target/` costs nothing here.
    fn note_dir(&mut self, path: &Path, dirs: &mut Vec<PathBuf>) {
        if self.ignore.is_ignored(path) {
            return;
        }
        let filter = self.tree.as_ref().map(Tree::filter).unwrap_or_default();
        let kind = if self.workspace.root().join(path).is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        if filter == Filter::Visible && self.workspace.is_ignored(path, kind) {
            return;
        }
        let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }

    /// The root-relative `relative` vanished: a loaded document keeps its
    /// last content under a banner (ADR 0028); a directory takes every
    /// document under it along.
    fn on_removed(&mut self, relative: &Path) {
        if self.queue.remove(relative) {
            tracing::info!(path = %relative.display(), "changed file went away");
        }
        let root = self.workspace.root().to_path_buf();
        for index in 0..self.docs.len() {
            let doc = &mut self.docs[index];
            if !(doc.relative == relative || doc.relative.starts_with(relative))
                || doc.deleted.is_some()
                || root.join(&doc.relative).is_file()
            {
                continue;
            }
            tracing::info!(path = %doc.relative.display(), "open file deleted; keeping its last content");
            doc.deleted = Some(Deleted::Banner);
            if self.current == Some(index) {
                self.notice(format!("{} was deleted", relative.display()));
            }
        }
    }

    /// Root-relative `from` became `to`: threads move with it, and a
    /// loaded document follows with its view intact (ADR 0028).
    fn on_renamed(&mut self, from: &Path, to: &Path) {
        tracing::info!(from = %from.display(), to = %to.display(), "renamed");
        let root = self.workspace.root().to_path_buf();
        let is_dir = root.join(to).is_dir();
        let moved = |path: &Path| -> Option<PathBuf> {
            if path == from {
                Some(to.to_path_buf())
            } else if is_dir {
                path.strip_prefix(from).ok().map(|rest| to.join(rest))
            } else {
                None
            }
        };
        self.queue.remove(from);
        let mut current_moved = None;
        let mut renamed = Vec::new();
        for index in 0..self.docs.len() {
            let Some(target) = moved(&self.docs[index].relative) else {
                continue;
            };
            let doc = &mut self.docs[index];
            doc.relative.clone_from(&target);
            doc.document.rename(root.join(&target));
            doc.deleted = None;
            if self.current == Some(index) {
                current_moved = Some(target);
            }
            renamed.push(index);
        }
        // The store moves first, so the marks are read once under the new
        // path rather than emptied and refilled.
        self.move_threads(&moved);
        for index in renamed {
            self.refresh_base(index);
            self.refresh_marks(index);
        }
        if let Some(target) = current_moved {
            self.notice(format!("renamed to {}", target.display()));
        }
    }

    /// The fingerprint the file at absolute `path` last had, from its
    /// loaded text or its last-seen snapshot, for rename pairing.
    pub fn last_seen_fingerprint(&self, path: &Path) -> Option<Fingerprint> {
        let relative = path.strip_prefix(self.workspace.root()).ok()?;
        if let Some(text) = self
            .docs
            .iter()
            .find(|doc| doc.relative == relative)
            .and_then(|doc| doc.document.text())
        {
            return Some(Fingerprint::from_bytes(text.as_bytes()));
        }
        let text = self.seen.as_ref()?.text(relative).ok().flatten()?;
        Some(Fingerprint::from_bytes(text.as_bytes()))
    }

    // ----- git status (ADR 0017) -----

    /// Every uncommitted path, in path order.
    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Re-read the dirty set. Runs after the hint debounce, so a burst of
    /// writes costs one walk; a failure is reported and leaves the set as
    /// it was.
    fn refresh_status(&mut self) {
        match self.workspace.status() {
            Ok(status) => {
                if status != self.status {
                    tracing::info!(dirty = status.len(), "dirty set changed");
                }
                self.status = status;
            }
            Err(error) => self.notice(format!("git status: {error}")),
        }
    }

    /// `]g`: the next hunk in this file, or the first hunk of the next
    /// uncommitted file when this one's run out, wrapping.
    pub fn hunk_next(&mut self) {
        self.step_hunk(true);
    }

    /// `[g`: the previous hunk, crossing into the last hunk of the
    /// previous uncommitted file.
    pub fn hunk_prev(&mut self) {
        self.step_hunk(false);
    }

    fn step_hunk(&mut self, forward: bool) {
        let step = if forward {
            self.view_mut().next_hunk()
        } else {
            self.view_mut().prev_hunk()
        };
        match step {
            HunkStep::Moved => {}
            HunkStep::NoBase if self.current.is_none() => self.notice("no file open"),
            HunkStep::NoBase => self.notice("no diff base: not in a git repository"),
            HunkStep::Wrapped | HunkStep::Clean => self.step_dirty(forward, true),
        }
    }

    /// `]G`: the next uncommitted file in path order, at its first hunk.
    pub fn dirty_next(&mut self) {
        self.step_dirty(true, false);
    }

    /// `[G`: the previous uncommitted file, at its first hunk.
    pub fn dirty_prev(&mut self) {
        self.step_dirty(false, false);
    }

    /// Open the next (or previous) dirty file. `from_hunk` lands on the
    /// last hunk when stepping backwards, so `[g` walks hunks in order;
    /// `]G` always lands on the first.
    fn step_dirty(&mut self, forward: bool, from_hunk: bool) {
        let current = self.current.map(|i| self.docs[i].relative.clone());
        let next = if forward {
            self.status.after(current.as_deref())
        } else {
            self.status.before(current.as_deref())
        };
        let Some(entry) = next else {
            self.notice("nothing uncommitted");
            return;
        };
        let path = entry.path().to_path_buf();
        let is_current = current.as_deref() == Some(path.as_path());
        let wrapped = is_current
            || match (forward, current.as_deref()) {
                (true, Some(cur)) => path < *cur,
                (false, Some(cur)) => path > *cur,
                _ => false,
            };
        if !is_current {
            if !self.workspace.root().join(&path).is_file() {
                self.notice(format!("{} is deleted", path.display()));
                return;
            }
            self.close_popup();
            self.focus = Focus::View;
            self.open(&path);
        }
        let line = if forward || !from_hunk {
            self.view().first_hunk_line()
        } else {
            self.view().last_hunk_line()
        };
        self.view_mut().goto_source_line(line.unwrap_or(1));
        if wrapped {
            self.notice(if forward {
                "wrapped to first change"
            } else {
                "wrapped to last change"
            });
        }
    }

    fn on_change(&mut self, relative: &Path, absolute: &Path) {
        let loaded = self.docs.iter().position(|doc| doc.relative == relative);
        // Ignore rules come before the `stat`: a build writing under
        // `target/` must cost one cached lookup per path, nothing more.
        if loaded.is_none()
            && (self.workspace.is_ignored(relative, EntryKind::File)
                || self.ignore.is_ignored(relative))
        {
            return;
        }
        if !absolute.is_file() {
            if self.queue.remove(relative) {
                tracing::info!(path = %relative.display(), "changed file went away");
            }
            return;
        }
        // A deleted file that came back: the banner goes and the reload
        // below re-anchors its threads (ADR 0028).
        if let Some(index) = loaded
            && self.docs[index].deleted.take().is_some()
        {
            tracing::info!(path = %relative.display(), "deleted file is back");
            if self.current == Some(index) {
                self.notice(format!("{} is back", relative.display()));
            }
        }
        let diff = loaded.and_then(|index| self.reload_doc(index));
        if loaded.is_some() && diff.is_none() {
            // The event did not change the text (a touch, or our own
            // write); nothing to hint about.
            return;
        }
        if self.workspace.is_ignored(relative, EntryKind::File) || self.ignore.is_ignored(relative)
        {
            return;
        }
        let (line, counts) = match (loaded, diff) {
            (Some(index), Some(diff)) => (
                diff.hunks()
                    .first()
                    .map(|hunk| hunk.target_line(diff.new_lines()))
                    .or_else(|| self.docs[index].view.first_hunk_line()),
                diff.counts(),
            ),
            _ => self.unloaded_change(relative, absolute),
        };
        let path = relative.to_path_buf();
        self.push_change(Change::new(path, Target::Line(line.unwrap_or(1))), counts);
    }

    /// The first hunk and counts of a file that is not open, against
    /// `HEAD` (ADR 0017), or against its last-seen snapshot outside git.
    fn unloaded_change(&self, relative: &Path, absolute: &Path) -> (Option<usize>, (usize, usize)) {
        let Ok(text) = fs::read_to_string(absolute) else {
            return (None, (0, 0));
        };
        let base = self
            .workspace
            .head_text(relative)
            .ok()
            .flatten()
            .or_else(|| self.seen.as_ref()?.text(relative).ok().flatten());
        let Some(base) = base else {
            return (None, (0, 0));
        };
        let diff = Diff::new(&base, &text);
        let line = diff
            .hunks()
            .first()
            .map(|hunk| hunk.target_line(diff.new_lines()));
        (line, diff.counts())
    }

    fn push_change(&mut self, change: Change, counts: (usize, usize)) {
        tracing::info!(path = %change.path.display(), line = change.target.line(), "change queued");
        if self.jump.toast > Duration::ZERO {
            let (added, removed) = counts;
            let text = if added == 0 && removed == 0 {
                change.path.display().to_string()
            } else {
                format!("{} +{added} -{removed}", change.path.display())
            };
            self.push_toast(text);
        }
        self.queue.push(change);
        self.last_change = Some(Instant::now());
    }

    /// Drop the current file from the queue once its target is on screen.
    pub fn settle(&mut self) {
        let Some(index) = self.current else {
            return;
        };
        let doc = &self.docs[index];
        let target = self
            .queue
            .iter()
            .find(|c| c.path == doc.relative)
            .map(|c| c.target.line());
        if let Some(line) = target
            && doc.view.line_on_screen(line)
        {
            self.queue.remove(&doc.relative.clone());
        }
    }

    /// Expire toasts, snapshot the current file once it has been idle long
    /// enough, and auto-jump when the guardrails allow it.
    pub fn tick(&mut self) {
        let now = Instant::now();
        self.toasts.retain(|toast| toast.until > now);
        if let Some(index) = self.current
            && self.docs[index].seen_dirty
            && self.docs[index].view.idle() >= self.viewer.seen_idle
        {
            self.mark_seen(index);
        }
        self.auto_jump_tick();
    }

    /// How long until [`App::tick`] has something to do, `None` when
    /// nothing is pending.
    pub fn tick_in(&self) -> Option<Duration> {
        let now = Instant::now();
        let mut next: Option<Duration> = None;
        let mut consider = |d: Duration| {
            next = Some(next.map_or(d, |n| n.min(d)));
        };
        if let Some(toast) = self.toasts.first() {
            consider(toast.until.saturating_duration_since(now));
        }
        if let Some(index) = self.current
            && self.docs[index].seen_dirty
        {
            let idle = self.docs[index].view.idle();
            consider(self.viewer.seen_idle.saturating_sub(idle));
        }
        if let Some(wait) = self.auto_jump_in() {
            consider(wait);
        }
        next
    }

    /// Snapshot the document at `index` as seen.
    pub(super) fn mark_seen(&mut self, index: usize) {
        let Some(doc) = self.docs.get_mut(index) else {
            return;
        };
        doc.seen_dirty = false;
        let (Some(seen), Some(text)) = (self.seen.as_mut(), doc.document.text()) else {
            return;
        };
        match seen.record(&doc.relative, text) {
            Ok(true) => tracing::debug!(path = %doc.relative.display(), "snapshotted as seen"),
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "cannot snapshot as seen"),
        }
    }

    /// Snapshot the visible file before the session ends.
    pub fn on_quit(&mut self) {
        if let Some(index) = self.current {
            self.mark_seen(index);
        }
    }

    /// The follow state for `session_info`.
    fn follow_state(&self) -> FollowState {
        FollowState {
            auto_jump: self.auto,
        }
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// Files an agent is following, in the order it gave them.
    pub fn followed(&self) -> &[PathBuf] {
        &self.followed
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    /// A click lands in a pane: it takes the keys, when it is on screen.
    pub fn focus_pane(&mut self, focus: Focus) {
        let present = match focus {
            Focus::View => true,
            Focus::Sidebar => self.tree().is_some(),
            Focus::Review => self.list.is_open(),
            Focus::ThreadsPane => self.threads_pane_height() > 0,
        };
        if present {
            self.focus = focus;
        }
    }

    /// Whether a document is open, rather than the welcome screen.
    pub fn has_document(&self) -> bool {
        self.current.is_some()
    }

    /// The border a drag is moving, while the button is down.
    pub fn dragging(&self) -> Option<Border> {
        self.drag
    }

    /// The mouse went down on a border.
    pub fn begin_drag(&mut self, border: Border) {
        self.drag = Some(border);
    }

    /// The mouse moved with a border held: the tree's divider follows the
    /// column, the thread pane's rule follows the row.
    pub fn drag_to(&mut self, column: usize, row: usize) {
        match self.drag {
            Some(Border::Sidebar) => self.sidebar_cols = Some(column + 1),
            Some(Border::Compose) => self.compose_rows = Some(self.pane_rows().saturating_sub(row)),
            Some(Border::ThreadsPane) => self.drag_threads_pane_to(row),
            None => return,
        }
        self.relayout();
    }

    pub fn end_drag(&mut self) {
        self.drag = None;
    }

    pub fn popup(&self) -> Option<&Popup> {
        self.popup.as_ref()
    }

    pub fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref().filter(|_| self.rail.tree)
    }

    pub fn sidebar_scroll(&self) -> usize {
        self.sidebar_scroll
    }

    /// The notice on the status line: the answer to the reader's last key,
    /// from the app or from the view. A toast, by contrast, reports what
    /// happened without the reader (ADR 0010).
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref().or_else(|| self.view().message())
    }

    /// The keys typed so far of a binding that is not complete.
    pub fn prefix(&self) -> &[Chord] {
        &self.prefix
    }

    /// Remember `typed` as the start of a longer binding.
    pub fn set_prefix(&mut self, typed: Vec<Chord>) {
        self.prefix = typed;
    }

    /// Take the pending keys, leaving none.
    pub fn take_prefix(&mut self) -> Vec<Chord> {
        std::mem::take(&mut self.prefix)
    }

    /// The visible view: the current document's or the welcome text.
    pub fn view(&self) -> &View {
        self.current
            .and_then(|i| self.docs.get(i))
            .map_or(&self.welcome, |doc| &doc.view)
    }

    pub fn view_mut(&mut self) -> &mut View {
        match self.current.and_then(|i| self.docs.get_mut(i)) {
            Some(doc) => &mut doc.view,
            None => &mut self.welcome,
        }
    }

    /// Root-relative path of the current document, empty when none is open.
    pub fn current_path(&self) -> &Path {
        self.current
            .and_then(|i| self.docs.get(i))
            .map_or(Path::new(""), |doc| &doc.relative)
    }

    /// Absolute path of the current document, for the watcher.
    pub fn current_abs_path(&self) -> Option<&Path> {
        self.current
            .and_then(|i| self.docs.get(i))
            .map(|doc| doc.document.path())
    }

    /// Whether the current document's file is gone from disk (ADR 0028).
    pub fn deleted(&self) -> bool {
        self.current
            .and_then(|i| self.docs.get(i))
            .is_some_and(|doc| doc.deleted.is_some())
    }

    /// The banner row over the text: `deleted` while the current file is
    /// gone and its last content is still shown (ADR 0028).
    pub fn banner(&self) -> Option<&'static str> {
        self.current
            .and_then(|i| self.docs.get(i))
            .filter(|doc| doc.deleted == Some(Deleted::Banner) && !self.list.is_open())
            .map(|_| "deleted")
    }

    /// Whether the current document is shown as the file-info pane
    /// because it was deleted and shown again while gone (ADR 0028).
    pub(super) fn deleted_info(&self) -> bool {
        self.current
            .and_then(|i| self.docs.get(i))
            .is_some_and(|doc| doc.deleted == Some(Deleted::Info))
    }

    /// The rail's width in columns, 0 when neither of its panes is shown
    /// (ADR 0049).
    pub fn rail_width(&self) -> usize {
        if !self.rail.tree && !self.rail.threads {
            return 0;
        }
        let widest = self.width.saturating_sub(TEXT_MIN_WIDTH);
        self.sidebar_cols
            .map_or_else(
                || self.rail.config.width.min(self.width / 3),
                |cols| cols.min(widest),
            )
            .max(SIDEBAR_MIN_WIDTH)
    }

    /// Rows available to panes once the status line is taken.
    pub fn pane_rows(&self) -> usize {
        self.height.saturating_sub(1).max(1)
    }

    /// Rows the comment box takes along the bottom, 0 when closed: its
    /// wrapped text plus the rule and header, capped, unless its rule was
    /// dragged (ADR 0018).
    pub fn compose_rows(&self) -> usize {
        let Some(Popup::Compose(compose)) = &self.popup else {
            return 0;
        };
        let tallest = self.pane_rows().saturating_sub(1);
        let wanted = self.compose_rows.unwrap_or_else(|| {
            (compose.buffer().rows(self.compose_width()).len() + 2).min(COMPOSE_MAX_ROWS)
        });
        wanted.clamp(COMPOSE_MIN_ROWS.min(tallest), tallest)
    }

    /// Columns the comment's text wraps at: the text column less the
    /// one-space margin.
    pub fn compose_width(&self) -> usize {
        self.width
            .saturating_sub(self.rail_width())
            .saturating_sub(1)
            .max(1)
    }

    /// The first wrapped row the comment box shows, chosen so the cursor's
    /// row is visible.
    pub fn compose_first_row(&self) -> usize {
        let Some(Popup::Compose(compose)) = &self.popup else {
            return 0;
        };
        let width = self.compose_width();
        let body = self.compose_rows().saturating_sub(2).max(1);
        let total = compose.buffer().rows(width).len();
        compose
            .buffer()
            .cursor_cell(width)
            .row
            .saturating_sub(body - 1)
            .min(total.saturating_sub(body))
    }

    /// A click in the comment box's text: `row` counts from the first
    /// visible wrapped row, `column` from the box's left edge.
    pub fn compose_click(&mut self, row: usize, column: usize) {
        let width = self.compose_width();
        let cell = Cell {
            row: self.compose_first_row() + row,
            column: column.saturating_sub(1),
        };
        if let Some(Popup::Compose(compose)) = &mut self.popup {
            compose.place_cursor(width, cell);
        }
    }

    /// Bracketed paste: into the comment box, else nothing to paste into.
    pub fn paste(&mut self, text: &str) {
        if matches!(self.popup, Some(Popup::Compose(_))) {
            self.compose_insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
        }
    }

    /// Rows left to the text once the thread pane is taken.
    pub fn text_rows(&self) -> usize {
        self.pane_rows()
            .saturating_sub(usize::from(self.banner().is_some()))
            .max(1)
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.relayout();
    }

    fn relayout(&mut self) {
        let rows = self.text_rows();
        let sidebar = self.rail_width();
        let text_width = self
            .width
            .saturating_sub(sidebar)
            .saturating_sub(crate::app::draw::gutter_width(self.view()))
            .max(1);
        self.view_mut().resize(text_width, rows);
        self.scroll_sidebar();
    }

    pub fn clear_message(&mut self) {
        self.message = None;
    }

    /// Raise a toast for `follow.toast`, dropping the oldest past the cap.
    pub(super) fn push_toast(&mut self, text: String) {
        if self.jump.toast == Duration::ZERO {
            return;
        }
        self.toasts.push(Toast {
            text,
            until: Instant::now() + self.jump.toast,
        });
        if self.toasts.len() > MAX_TOASTS {
            self.toasts.remove(0);
        }
    }

    fn notice(&mut self, message: impl Into<String>) {
        let message = message.into();
        tracing::info!(message = %message, "status");
        self.message = Some(message);
    }

    /// Open the root-relative `path`, loading it or switching to it.
    pub fn open(&mut self, path: &Path) {
        let relative = self.workspace.relative(&self.workspace.root().join(path));
        let loaded = self.docs.iter().position(|doc| doc.relative == relative);
        let index = if let Some(index) = loaded {
            index
        } else {
            let absolute = self.workspace.root().join(&relative);
            let policy = Policy {
                attr: self.workspace.diff_attr(&relative),
                max_bytes: self.viewer.max_file_bytes(),
            };
            match Document::load(&absolute, policy) {
                Ok(document) => {
                    // A binary or over-limit file has no text: the view is
                    // empty and the file-info pane draws instead (ADR 0026).
                    let view = View::with_syntax(
                        document.text().unwrap_or_default().to_owned(),
                        1,
                        1,
                        self.syntax_for(&relative),
                    );
                    self.docs.push(Doc {
                        document,
                        relative: relative.clone(),
                        view,
                        marks: Vec::new(),
                        seen_dirty: true,
                        deleted: None,
                    });
                    self.docs.len() - 1
                }
                Err(error) => {
                    self.notice(format!("{error:#}"));
                    return;
                }
            }
        };
        self.recent.retain(|&recent| recent != index);
        self.recent.insert(0, index);
        self.show(index);
    }

    /// Answer a socket request that needs app state (ADR 0014).
    pub fn handle_request(&mut self, request: Request) -> Response {
        match request {
            Request::Ping => Response::Pong,
            Request::SessionInfo => {
                Response::Session(self.record.clone(), Some(self.follow_state()))
            }
            Request::Open {
                path,
                line,
                end_line,
            } => self.open_for_agent(&path, line, end_line),
            Request::Follow { paths } => {
                tracing::info!(count = paths.len(), "agent follow list replaced");
                self.followed = paths;
                self.reveal_followed();
                Response::Done
            }
            Request::AnnotationsList { since, path } => match &self.store {
                Some(store) => Response::Threads(
                    store
                        .threads()
                        .iter()
                        .filter(|t| self.reach.includes(t))
                        .filter(|t| since.is_none_or(|s| t.updated() >= s))
                        .filter(|t| path.as_deref().is_none_or(|p| t.path().starts_with(p)))
                        .cloned()
                        .collect(),
                ),
                None => Response::Error("threads unavailable; see the log".to_owned()),
            },
            Request::ThreadReply {
                thread,
                author,
                body,
                resolve,
                lines,
            } => match self.agent_reply(&thread, author, body, resolve, lines) {
                Ok(()) => Response::Done,
                Err(error) => Response::Error(error),
            },
        }
    }

    /// `open` from an agent: the path must stay inside the workspace.
    fn open_for_agent(
        &mut self,
        path: &Path,
        line: Option<usize>,
        end_line: Option<usize>,
    ) -> Response {
        let inside = path.is_relative()
            && path
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if !inside {
            return Response::Error(format!(
                "{} is not a workspace-relative path",
                path.display()
            ));
        }
        if !self.workspace.root().join(path).is_file() {
            return Response::Error(format!("{} is not a file in the workspace", path.display()));
        }
        self.close_popup();
        self.focus = Focus::View;
        self.record_jump_from_here();
        self.open(path);
        if self.current_path() != path {
            return Response::Error(
                self.message
                    .clone()
                    .unwrap_or_else(|| format!("cannot open {}", path.display())),
            );
        }
        // An agent `open` is a change with an explicit target in every
        // source mode (ADR 0015); it is on screen at once, so it settles.
        let target = match (line, end_line) {
            (Some(start), Some(end)) if end > start => Target::Range(start, end),
            (Some(start), _) => Target::Line(start),
            (None, _) => Target::Line(1),
        };
        self.queue.push(Change::new(path.to_path_buf(), target));
        self.last_change = None;
        // The range is shown, not selected: a selection would sit in the
        // reader's way (ADR 0014, amended 2026-08-28).
        if let Some(line) = line {
            let view = self.view_mut();
            view.escape();
            view.reveal_source_range(line, end_line.unwrap_or(line).max(line));
        }
        Response::Done
    }

    fn show(&mut self, index: usize) {
        if let Some(previous) = self.current
            && previous != index
        {
            self.mark_seen(previous);
        }
        self.current = Some(index);
        // Shown again while still gone: the file-info pane says so
        // (ADR 0028).
        if let Some(doc) = self.docs.get_mut(index)
            && doc.deleted.is_some()
            && !doc.document.path().is_file()
        {
            doc.deleted = Some(Deleted::Info);
        }
        // A document takes the column back from the list (ADR 0025).
        self.list.close();
        self.focus = Focus::View;
        self.refresh_base(index);
        self.refresh_marks(index);
        // The document's stubs follow the session's toggles (ADR 0049).
        self.place_stub_rows();
        self.relayout();
        tracing::info!(path = %self.current_path().display(), "showing document");
    }

    /// Where the reader is: the current file and the cursor's source
    /// line, or `None` before any file is open.
    fn position(&self) -> Option<jumplist::Position> {
        let path = self.current_path().to_path_buf();
        if path.as_os_str().is_empty() {
            return None;
        }
        let line = self.view().cursor_source_line().unwrap_or(1);
        Some(jumplist::Position { path, line })
    }

    /// A far move is leaving `from` (ADR 0049).
    pub(crate) fn record_jump(&mut self, from: jumplist::Position) {
        self.jumplist.record(from);
    }

    /// A far move that does not come through a key — auto-jump, an
    /// agent's `open` — records where it is leaving from.
    fn record_jump_from_here(&mut self) {
        if let Some(from) = self.position() {
            self.record_jump(from);
        }
    }

    /// The position a far move would record now.
    pub(crate) fn jump_origin(&self) -> Option<jumplist::Position> {
        self.position()
    }

    /// `Alt-Left`: the previous position in the jumplist.
    pub fn jump_back(&mut self) {
        let Some(here) = self.position() else {
            return;
        };
        match self.jumplist.back(here).cloned() {
            Some(target) => self.go_to_position(&target),
            None => self.notice("at oldest position"),
        }
    }

    /// `Alt-Right`: the next position in the jumplist.
    pub fn jump_forward(&mut self) {
        match self.jumplist.forward().cloned() {
            Some(target) => self.go_to_position(&target),
            None => self.notice("at newest position"),
        }
    }

    fn go_to_position(&mut self, target: &jumplist::Position) {
        if self.current_path() != target.path {
            self.close_popup();
            self.open(&target.path);
            if self.current_path() != target.path {
                return;
            }
        }
        self.view_mut().goto_source_line(target.line);
    }

    /// Re-read the document at `index`; the diff from the text that was
    /// on screen to the text that replaced it, if they differ.
    fn reload_doc(&mut self, index: usize) -> Option<Diff> {
        let doc = &mut self.docs[index];
        match doc.document.reload() {
            Ok(true) => {
                tracing::info!(path = %doc.relative.display(), "reloaded after change");
                let old = doc.view.text().to_owned();
                let text = doc.document.text().unwrap_or_default();
                let diff = Diff::new(&old, text);
                doc.view.reload(text.to_owned());
                doc.seen_dirty = true;
                self.refresh_base(index);
                self.remap_marks(index, &old);
                Some(diff)
            }
            Ok(false) => None,
            Err(error) => {
                tracing::warn!(%error, "reload failed; keeping previous text");
                None
            }
        }
    }

    /// Re-read the diff bases of the document at `index`: the last-seen
    /// snapshot (ADR 0015) and the `HEAD` text (ADR 0006). A `HEAD` read
    /// failure is reported once and leaves no base.
    fn refresh_base(&mut self, index: usize) {
        // A binary or over-limit file has nothing to diff (ADR 0026).
        let Some(relative) = self
            .docs
            .get(index)
            .filter(|doc| doc.document.text().is_some())
            .map(|doc| doc.relative.clone())
        else {
            return;
        };
        let head = match self.workspace.head_text(&relative) {
            Ok(base) => base,
            Err(error) => {
                self.notice(format!("no diff base: {error}"));
                None
            }
        };
        let staged = match self.workspace.index_text(&relative) {
            Ok(base) => base,
            Err(error) => {
                tracing::warn!(%error, "cannot read the index text; hunks show as unstaged");
                None
            }
        };
        let seen = self
            .seen
            .as_ref()
            .and_then(|seen| match seen.text(&relative) {
                Ok(text) => text,
                Err(error) => {
                    tracing::warn!(%error, "cannot read last-seen snapshot");
                    None
                }
            });
        if let Some(doc) = self.docs.get_mut(index) {
            doc.view.set_bases(seen, staged, head);
        }
    }

    // ----- sidebar -----

    fn ensure_tree(&mut self) -> bool {
        if self.tree.is_some() {
            return true;
        }
        match Tree::new(&mut self.workspace) {
            Ok(tree) => {
                self.tree = Some(tree);
                true
            }
            Err(error) => {
                self.notice(error.to_string());
                false
            }
        }
    }

    /// What a start shows: the file named, else the rail with both panes
    /// and the tree focused (ADR 0012, ADR 0049).
    pub fn start_on(&mut self, open: Option<&Path>) {
        if let Some(path) = open {
            self.open(path);
        } else {
            self.show_sidebar();
            self.show_threads_pane();
        }
    }

    /// Show and focus the tree, as a workspace start does (ADR 0012).
    pub fn show_sidebar(&mut self) {
        if !self.rail.tree {
            self.toggle_sidebar_focus();
        }
    }

    /// `Space e`: open and focus the tree, or hand focus back.
    pub fn toggle_sidebar_focus(&mut self) {
        if !self.rail.tree {
            if !self.ensure_tree() {
                return;
            }
            self.rail.tree = true;
            self.reveal_current();
            self.focus = Focus::Sidebar;
        } else if self.focus == Focus::Sidebar {
            self.focus = Focus::View;
        } else {
            self.reveal_current();
            self.focus = Focus::Sidebar;
        }
        self.relayout();
    }

    /// `Space E`: hide the tree pane; the threads pane keeps the rail.
    pub fn hide_sidebar(&mut self) {
        self.rail.tree = false;
        if self.focus == Focus::Sidebar {
            self.focus = Focus::View;
        }
        self.relayout();
    }

    fn reveal_current(&mut self) {
        let path = self.current_path().to_path_buf();
        if path.as_os_str().is_empty() {
            return;
        }
        if let Some(tree) = self.tree.as_mut()
            && let Err(error) = tree.reveal(&mut self.workspace, &path)
        {
            tracing::debug!(%error, "cannot reveal current file in tree");
        }
    }

    /// Show every followed file the tree does not list yet (ADR 0028):
    /// its parents expand without moving the cursor, unless the sidebar
    /// has focus, in which case the cursor lands on it and pages the
    /// viewer to it as the tree keys do (ADR 0023).
    fn reveal_followed(&mut self) {
        if self.tree.is_none() {
            return;
        }
        for path in self.followed.clone() {
            if self.tree.as_ref().is_some_and(|tree| tree.contains(&path)) {
                continue;
            }
            if self.focus == Focus::Sidebar {
                self.with_tree_result(|tree, workspace| {
                    tree.reveal(workspace, &path).map(|_| None)
                });
                self.show_highlight();
            } else {
                self.with_tree_result(|tree, workspace| {
                    tree.expand_to(workspace, &path).map(|_| None)
                });
            }
        }
    }

    // ----- popups -----

    pub fn open_help(&mut self) {
        self.popup = Some(Popup::Help);
    }

    /// `:status`: the overlay of session facts (ADR 0021).
    pub fn open_status(&mut self) {
        self.popup = Some(Popup::Status);
    }

    pub fn close_popup(&mut self) {
        self.popup = None;
    }

    pub fn open_picker(&mut self, kind: PickerKind) {
        let items = match kind {
            PickerKind::Files => self.index(Filter::Visible),
            PickerKind::AllFiles => self.index(Filter::All),
            PickerKind::Recent => self
                .recent
                .iter()
                .map(|&index| self.docs[index].relative.to_string_lossy().into_owned())
                .collect(),
            PickerKind::Wake => self
                .subscribers()
                .iter()
                .map(|s| format!("{}  {}", s.label(), s.id()))
                .collect(),
        };
        tracing::info!(?kind, items = items.len(), "picker opened");
        self.popup = Some(Popup::Picker(PickerState::new(kind, items)));
    }

    fn index(&mut self, filter: Filter) -> Vec<String> {
        let slot = match filter {
            Filter::All => &mut self.all_index,
            Filter::Visible => &mut self.file_index,
        };
        if slot.is_none() {
            let files = self.workspace.walk_files(filter);
            tracing::info!(files = files.len(), ?filter, "indexed workspace");
            *slot = Some(files);
        }
        slot.clone().unwrap_or_default()
    }

    fn picker_mut(&mut self) -> Option<&mut PickerState> {
        match self.popup.as_mut() {
            Some(Popup::Picker(state)) => Some(state),
            _ => None,
        }
    }

    pub fn picker_char(&mut self, ch: char) {
        if let Some(picker) = self.picker_mut() {
            picker.input.push(ch);
            picker.requery();
        }
    }

    pub fn picker_backspace(&mut self) {
        if let Some(picker) = self.picker_mut() {
            picker.input.pop();
            picker.requery();
        }
    }

    pub fn picker_move(&mut self, delta: isize) {
        if let Some(picker) = self.picker_mut() {
            let last = picker.matches.len().saturating_sub(1);
            picker.selected = picker.selected.saturating_add_signed(delta).min(last);
        }
    }

    /// Enter in the picker: open the file, or show the thread.
    pub fn picker_confirm(&mut self) {
        let choice = self.picker_mut().and_then(|picker| {
            let m = picker.matches.get(picker.selected)?;
            Some((picker.kind, picker.item(m).to_owned()))
        });
        self.popup = None;
        match choice {
            Some((PickerKind::Files | PickerKind::AllFiles | PickerKind::Recent, path)) => {
                self.open(Path::new(&path));
            }
            Some((PickerKind::Wake, item)) => {
                if let Some(id) = item.rsplit("  ").next() {
                    self.wake_subscriber(id);
                }
            }
            None => {}
        }
    }
}

/// Everything [`App::new`] needs beyond the workspace and terminal size.
#[derive(Debug)]
pub struct Options {
    /// This viewer's record, already written to the sessions directory.
    pub record: Record,
    /// Where records and state live, for `:name` to rewrite the record.
    pub dirs: XdgDirs,
    /// The workspace's thread store, or `None` when it could not be opened.
    pub store: Option<Store>,
    /// Auto-jump settings (ADR 0015).
    pub jump: JumpConfig,
    /// File-watcher settings (ADR 0015).
    pub watch: WatchConfig,
    /// The last-seen snapshot store, or `None` when it could not be opened.
    pub seen: Option<seen::Store>,
    /// Code highlighting for fences and source files (ADR 0016).
    pub highlighter: Arc<Highlighter>,
    /// Which files render as Markdown (ADR 0016).
    pub markdown: MarkdownConfig,
    /// How files are read (ADR 0026).
    pub viewer: ViewerConfig,
    /// The rail's width and split (ADR 0049).
    pub rail: RailConfig,
    /// How threads show in the text (ADR 0049).
    pub threads: ThreadsConfig,
    /// Subscriptions and the wake command (ADR 0040).
    pub agents: AgentsConfig,
    /// The config file in use, for the over-limit notice (ADR 0026).
    pub config_path: PathBuf,
}

#[cfg(test)]
impl Options {
    /// Options for a test app rooted at `root`: no stores, plain code.
    pub(crate) fn for_test(root: PathBuf) -> Self {
        use fathomable_core::session::Id;
        Self {
            record: Record::new(Id::mint(), root, None),
            dirs: XdgDirs::resolve(|_| None),
            store: None,
            jump: JumpConfig::default(),
            watch: WatchConfig::default(),
            seen: None,
            highlighter: Arc::new(Highlighter::plain()),
            markdown: MarkdownConfig::default(),
            viewer: ViewerConfig::default(),
            rail: RailConfig::default(),
            threads: ThreadsConfig::default(),
            agents: AgentsConfig::default(),
            config_path: PathBuf::from("config.kdl"),
        }
    }
}

#[cfg(test)]
mod tests {
    use fathomable_testing::{TempDir, git};
    use std::fs;
    use std::path::{Path, PathBuf};

    use std::sync::Arc;

    use fathomable_core::config::{JumpConfig, MarkdownConfig, ViewerConfig, WatchConfig};
    use fathomable_core::highlight::Highlighter;
    use fathomable_core::tree::Tree;
    use fathomable_core::workspace::{Workspace, WorkspaceError};

    use super::input::bindings::Action;
    use super::{App, Focus, Options, PickerKind, Popup};

    fn fixture(name: &str) -> std::io::Result<TempDir> {
        let dir = TempDir::new(&format!("app-{name}"))?;
        fs::create_dir_all(dir.0.join("docs"))?;
        fs::write(dir.0.join("README.md"), "# Readme\n\nhello\n")?;
        fs::write(dir.0.join("docs/guide.md"), "# Guide\n")?;
        fs::write(dir.0.join("docs/notes.md"), "# Notes\n")?;
        Ok(dir)
    }

    fn app(dir: &TempDir) -> Result<App, WorkspaceError> {
        app_with(dir, Options::for_test(dir.0.clone()))
    }

    fn app_with(dir: &TempDir, options: Options) -> Result<App, WorkspaceError> {
        let workspace = Workspace::discover(&dir.0)?;
        Ok(App::new(workspace, 100, 30, options))
    }

    fn picker_items(app: &App) -> Vec<String> {
        match app.popup() {
            Some(Popup::Picker(picker)) => picker
                .matches()
                .iter()
                .map(|m| picker.item(m).to_owned())
                .collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn source_files_open_highlighted_and_markdown_files_rendered() -> anyhow::Result<()> {
        let dir = fixture("syntax")?;
        fs::write(dir.0.join("main.rs"), "fn main() {}\n")?;
        fs::write(dir.0.join("LICENSE"), "# Terms\n")?;
        fs::write(dir.0.join("justfile"), "default:\n    make help\n")?;
        let mut app = app_with(
            &dir,
            Options {
                highlighter: Arc::new(Highlighter::new("base16-ocean.dark")?),
                ..Options::for_test(dir.0.clone())
            },
        )?;
        app.open(Path::new("main.rs"));
        assert!(app.view().source_view(), "a .rs file opens as source");
        let coloured = app.view().layout().lines()[0]
            .spans()
            .iter()
            .any(|span| span.style().fg.is_some());
        assert!(coloured, "the source layout is highlighted by extension");
        app.open(Path::new("README.md"));
        assert!(!app.view().source_view(), "Markdown opens rendered");
        assert_eq!(app.view().layout().lines()[0].text(), "Readme");
        app.open(Path::new("LICENSE"));
        assert!(
            !app.view().source_view(),
            "listed extensionless names render as Markdown"
        );
        app.open(Path::new("justfile"));
        assert!(
            app.view().source_view(),
            "unlisted extensionless files open as source"
        );

        // A narrower list flips both.
        let mut app = app_with(
            &dir,
            Options {
                markdown: MarkdownConfig {
                    extensions: vec!["rs".to_owned()],
                    names: Vec::new(),
                },
                ..Options::for_test(dir.0.clone())
            },
        )?;
        app.open(Path::new("LICENSE"));
        assert!(app.view().source_view());
        app.open(Path::new("main.rs"));
        assert!(!app.view().source_view());
        Ok(())
    }

    #[test]
    fn opening_files_builds_the_recent_list() -> anyhow::Result<()> {
        let dir = fixture("history")?;
        let mut app = app(&dir)?;
        assert_eq!(app.current_path(), Path::new(""));
        assert!(
            app.position().is_none(),
            "no position before a file is open"
        );
        app.open(Path::new("README.md"));
        app.open(Path::new("docs/guide.md"));
        assert_eq!(app.current_path(), Path::new("docs/guide.md"));
        app.open(Path::new("missing.md"));
        assert!(app.message().is_some_and(|m| m.contains("missing.md")));
        assert_eq!(app.current_path(), Path::new("docs/guide.md"));

        app.open_picker(PickerKind::Recent);
        assert_eq!(picker_items(&app), ["docs/guide.md", "README.md"]);
        Ok(())
    }

    /// A sidebar squeezed past the width of its narrowest name still draws
    /// whole rows: the git letter has no column to take, and the marks that
    /// no longer fit take no width either.
    #[test]
    fn narrow_sidebar_draws_whole_rows() -> anyhow::Result<()> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let dir = fixture("narrow")?;
        git::init(&dir.0)?;
        git::commit_and_stage(
            &dir.0,
            &[
                ("README.md", "# Readme\n"),
                (
                    "docs/deep/notes.md",
                    concat!("# Notes\n", "old\nold\nold\nold\nold\nold\nold\nold\n"),
                ),
            ],
        )?;
        // A modified file earns the git letter, and enough changed lines
        // earn `+n -m` counts wider than the sidebar itself.
        fs::write(dir.0.join("README.md"), "# Readme\n\nmore\n")?;
        fs::create_dir_all(dir.0.join("docs/deep"))?;
        fs::write(
            dir.0.join("docs/deep/notes.md"),
            "# Notes\n".to_owned() + &"line\n".repeat(400),
        )?;
        let mut app = app(&dir)?;
        // Opening the nested file unfolds the tree down to it, so the rows
        // are indented past what a narrow sidebar can show.
        app.open(Path::new("docs/deep/notes.md"));
        app.show_sidebar();
        assert!(
            app.tree()
                .is_some_and(|tree| tree.rows().iter().any(|row| row.depth() == 2)),
            "the tree is unfolded to the nested file"
        );

        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::draw::Theme::from_core(&core);
        for width in 1..=40u16 {
            app.resize(usize::from(width), 12);
            let mut terminal = Terminal::new(TestBackend::new(width, 12))?;
            terminal.draw(|frame| crate::app::draw::draw(frame, &app, &theme))?;
            let buffer = terminal.backend().buffer().clone();
            let divider = u16::try_from(app.rail_width())?.saturating_sub(1);
            if divider >= width {
                continue;
            }
            for y in 0..buffer.area.height - 1 {
                assert_eq!(
                    buffer[(divider, y)].symbol(),
                    "│",
                    "row {y} of a {width}-column terminal ends at the divider"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn sidebar_toggles_focus_and_reveals_current_file() -> anyhow::Result<()> {
        let dir = fixture("sidebar")?;
        let mut app = app(&dir)?;
        assert_eq!(app.rail_width(), 0);
        app.open(Path::new("docs/notes.md"));
        app.toggle_sidebar_focus();
        assert_eq!(app.focus(), Focus::Sidebar);
        assert_eq!(app.rail_width(), 32);
        let selected = app
            .tree()
            .and_then(|tree| tree.current())
            .map(|row| row.path().to_path_buf());
        assert_eq!(selected.as_deref(), Some(Path::new("docs/notes.md")));
        app.toggle_sidebar_focus();
        assert_eq!(app.focus(), Focus::View);
        assert!(app.tree().is_some(), "tree stays visible");
        app.hide_sidebar();
        assert!(app.tree().is_none());
        Ok(())
    }

    #[test]
    fn ge_goes_to_the_end_like_g_in_the_tree_and_the_view() -> anyhow::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use super::{Tree, input::keys};
        let dir = fixture("ge")?;
        let mut app = app(&dir)?;
        app.open(Path::new("README.md"));
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        keys::handle_key(&mut app, key('g'));
        keys::handle_key(&mut app, key('e'));
        let bottom = app.view().source_position().0;
        keys::handle_key(&mut app, key('g'));
        keys::handle_key(&mut app, key('g'));
        let top = app.view().source_position().0;
        assert!(bottom > top, "ge leaves the top line");
        keys::handle_key(&mut app, key('G'));
        assert_eq!(app.view().source_position().0, bottom, "ge matches G");

        app.toggle_sidebar_focus();
        keys::handle_key(&mut app, key('g'));
        keys::handle_key(&mut app, key('g'));
        let top = app
            .tree()
            .and_then(Tree::current)
            .map(|r| r.path().to_path_buf());
        keys::handle_key(&mut app, key('g'));
        keys::handle_key(&mut app, key('e'));
        let last = app
            .tree()
            .and_then(Tree::current)
            .map(|r| r.path().to_path_buf());
        assert_ne!(top, last, "ge leaves the first tree row");
        keys::handle_key(&mut app, key('j'));
        let after = app
            .tree()
            .and_then(Tree::current)
            .map(|r| r.path().to_path_buf());
        assert_eq!(last, after, "ge lands on the last tree row");
        Ok(())
    }

    #[test]
    fn long_lines_wrap_in_rendered_source_and_diff_views() -> anyhow::Result<()> {
        let dir = fixture("wrap")?;
        let long = "abcdefghijklmnopqrstuvwxyz".repeat(8);
        fs::write(dir.0.join("long.md"), format!("```\n{long}\n```\n"))?;
        let mut app = app(&dir)?;
        app.open(Path::new("long.md"));
        app.resize(40, 12);
        for display in 0..3 {
            let layout = app.view().layout();
            assert!(
                layout
                    .lines()
                    .iter()
                    .all(|line| line.width() <= layout.width()),
                "display {display} contains an overlong line"
            );
            match display {
                0 => app.view_mut().toggle_source_view(),
                1 => {
                    app.view_mut().set_bases(None, None, Some(String::new()));
                    app.view_mut().toggle_diff_view();
                }
                _ => {}
            }
        }
        Ok(())
    }

    #[test]
    fn the_tree_highlight_pages_the_viewer() -> anyhow::Result<()> {
        use crossterm::event::{
            KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
        };

        use super::input::keys;
        let dir = fixture("paging")?;
        let mut app = app(&dir)?;
        app.open(Path::new("README.md"));
        app.toggle_sidebar_focus();
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        // Directories come first, so `k` from README.md lands on `docs`.
        keys::handle_key(&mut app, key('k'));
        assert_eq!(
            app.current_path(),
            Path::new("README.md"),
            "a directory row leaves the pane on the file it shows"
        );
        keys::handle_key(&mut app, key('l'));
        keys::handle_key(&mut app, key('j'));
        assert_eq!(app.current_path(), Path::new("docs/guide.md"));
        assert_eq!(app.focus(), Focus::Sidebar, "paging does not steal focus");

        // The wheel steps one row per tick: guide.md to notes.md, not three
        // rows down.
        crate::app::input::mouse::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 0,
                row: 5,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        assert_eq!(app.focus(), Focus::Sidebar);

        // Paging is browsing, not a far move: the jumplist has nothing.
        app.jump_back();
        assert_eq!(app.message(), Some("at oldest position"));
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));

        // A click pages too: it shows the row it lands on and stays in
        // the tree. Row 0 is the root header, so screen row 4 is README.
        crate::app::input::mouse::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 0,
                row: 4,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.focus(), Focus::Sidebar, "a click does not steal focus");

        // Enter commits: focus moves to the viewer.
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.focus(), Focus::View);
        Ok(())
    }

    #[test]
    fn left_at_column_zero_hands_focus_to_the_tree() -> anyhow::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use super::input::keys;
        let dir = fixture("left")?;
        let mut app = app(&dir)?;
        app.open(Path::new("docs/notes.md"));
        let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
        keys::handle_key(&mut app, left);
        assert_eq!(app.focus(), Focus::View, "no tree, nothing to focus");
        app.toggle_sidebar_focus();
        app.toggle_sidebar_focus();
        assert_eq!(app.focus(), Focus::View);
        keys::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
        );
        keys::handle_key(&mut app, left);
        assert_eq!(app.focus(), Focus::View, "a selection keeps focus");
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        keys::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        );
        assert_eq!(app.focus(), Focus::Sidebar);
        Ok(())
    }

    #[test]
    fn picker_filters_and_opens() -> anyhow::Result<()> {
        let dir = fixture("picker")?;
        let mut app = app(&dir)?;
        app.act(Action::PickFile);
        assert_eq!(picker_items(&app).len(), 3);
        for ch in "guide".chars() {
            app.picker_char(ch);
        }
        assert_eq!(picker_items(&app), ["docs/guide.md"]);
        app.picker_confirm();
        assert!(app.popup().is_none());
        assert_eq!(app.current_path(), Path::new("docs/guide.md"));
        assert_eq!(app.focus(), Focus::View);
        Ok(())
    }

    #[test]
    fn unchanged_content_queues_nothing() -> anyhow::Result<()> {
        let dir = fixture("touch")?;
        let mut app = app(&dir)?;
        app.open(Path::new("README.md"));
        app.open(Path::new("docs/guide.md"));
        // A touch (or our own read, reported as a change) on an open file
        // whose text is identical must not hint.
        app.on_changes(vec![dir.0.join("README.md"), dir.0.join("README.md")]);
        assert!(app.queue().is_empty());
        Ok(())
    }

    /// A thread written against a commit HEAD does not contain is hidden
    /// from the marks and from `annotations_list`; one written against
    /// HEAD, or with no commit, shows. An append by another writer reaches
    /// the viewer through the store watch (ADR 0024).
    #[test]
    fn threads_follow_the_work_and_other_writers_are_picked_up() -> anyhow::Result<()> {
        use fathomable_core::annotations::{Draft, LineRange, Store};
        use fathomable_core::session::{Request, Response};

        let dir = fixture("scope")?;
        git::init(&dir.0)?;
        git::commit_and_stage(&dir.0, &[("README.md", "# Readme\n\nhello\n")])?;
        let workspace = Workspace::discover(&dir.0)?;
        let head = workspace
            .head_commit()
            .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
        let store_path = dir.0.join(".state/threads.jsonl");
        let text = "# Readme\n\nhello\n";
        let mut store = Store::open(&store_path)?;
        let here = store.annotate(
            Draft::new(Path::new("README.md"), LineRange::new(1, 1), "on this work")
                .at_commit(Some(head)),
            text,
            1,
        )?;
        // Written on lines this checkout does not have, as a thread from
        // another branch is; one whose lines are here would follow HEAD
        // (ADR 0035).
        store.annotate(
            Draft::new(
                Path::new("README.md"),
                LineRange::new(2, 2),
                "on other work",
            )
            .at_commit(Some("0123456789abcdef0123456789abcdef01234567".to_owned())),
            "# Readme\nelsewhere\nhello\n",
            2,
        )?;
        let legacy = store.annotate(
            Draft::new(Path::new("README.md"), LineRange::new(3, 3), "unscoped"),
            text,
            3,
        )?;

        let mut app = app_with(
            &dir,
            Options {
                store: Some(Store::open(&store_path)?),
                ..Options::for_test(dir.0.clone())
            },
        )?;
        app.open(Path::new("README.md"));
        let ids: Vec<_> = app.marks().iter().map(|m| m.id().clone()).collect();
        assert_eq!(ids, [here.clone(), legacy.clone()]);
        let Response::Threads(listed) = app.handle_request(Request::AnnotationsList {
            since: None,
            path: None,
        }) else {
            anyhow::bail!("expected threads");
        };
        assert_eq!(listed.len(), 2);

        // Another writer appends while this viewer runs.
        let late = store.annotate(
            Draft::new(Path::new("README.md"), LineRange::new(3, 3), "late"),
            text,
            4,
        )?;
        app.on_changes(vec![store_path.clone()]);
        let ids: Vec<_> = app.marks().iter().map(|m| m.id().clone()).collect();
        assert_eq!(ids, [here, legacy, late]);
        Ok(())
    }

    /// An amend replaces `HEAD` with a commit that does not descend from
    /// it. An open thread whose lines are still in the working tree moves
    /// to the new commit and stays visible; one whose lines are gone, and
    /// a resolved one, stay scoped to the dropped commit (ADR 0035).
    #[test]
    fn open_threads_follow_head_across_an_amend() -> anyhow::Result<()> {
        use fathomable_core::annotations::{Author, Draft, LineRange, Store, Thread};

        let dir = fixture("rescope")?;
        git::init(&dir.0)?;
        let text = "# Readme\n\nhello\n";
        git::commit_and_stage(&dir.0, &[("README.md", text)])?;
        fs::write(dir.0.join("README.md"), text)?;
        let workspace = Workspace::discover(&dir.0)?;
        let first = workspace
            .head_commit()
            .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
        let store_path = dir.0.join(".state/threads.jsonl");
        let mut store = Store::open(&store_path)?;
        let at = |line: usize, comment: &str| {
            Draft::new(Path::new("README.md"), LineRange::new(line, line), comment)
                .at_commit(Some(first.clone()))
        };
        let kept = store.annotate(at(1, "kept"), text, 1)?;
        let gone = store.annotate(at(3, "lines gone"), text, 2)?;
        let done = store.annotate(at(1, "resolved"), text, 3)?;
        store.resolve(&done, Author::User, 4)?;

        let mut app = app_with(
            &dir,
            Options {
                store: Some(Store::open(&store_path)?),
                ..Options::for_test(dir.0.clone())
            },
        )?;
        app.open(Path::new("README.md"));
        assert_eq!(app.marks().len(), 3);

        // Amend: an orphan commit with the third line dropped.
        let amended = "# Readme\n\n";
        git::amend(&dir.0, &[("README.md", amended)])?;
        changed(&mut app, &dir, "README.md", amended)?;
        app.on_changes(vec![dir.0.join(".git/HEAD")]);
        let second = app
            .workspace
            .head_commit()
            .ok_or_else(|| anyhow::anyhow!("no HEAD"))?;
        assert_ne!(first, second);

        let ids: Vec<_> = app.marks().iter().map(|m| m.id().clone()).collect();
        assert_eq!(ids, std::slice::from_ref(&kept));
        let again = Store::open(&store_path)?;
        assert_eq!(
            again.thread(&kept).and_then(Thread::commit),
            Some(second.as_str())
        );
        assert_eq!(
            again.thread(&gone).and_then(Thread::commit),
            Some(first.as_str())
        );
        assert_eq!(
            again.thread(&done).and_then(Thread::commit),
            Some(first.as_str())
        );
        Ok(())
    }

    fn changed(app: &mut App, dir: &TempDir, relative: &str, text: &str) -> std::io::Result<()> {
        let absolute = dir.0.join(relative);
        fs::write(&absolute, text)?;
        app.on_changes(vec![absolute]);
        Ok(())
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "one walk through the whole key set")]
    fn hunks_cross_uncommitted_files_in_path_order() -> anyhow::Result<()> {
        use fathomable_core::status::State;

        let dir = fixture("hunks")?;
        git::init(&dir.0)?;
        let committed = [
            ("README.md", "# Readme\n\nhello\n"),
            ("docs/guide.md", "# Guide\n"),
            ("docs/notes.md", "# Notes\n"),
        ];
        git::commit_and_stage(&dir.0, &committed)?;
        fs::write(
            dir.0.join("README.md"),
            "# Readme\n\nfirst\n\nhello\n\nlast\n",
        )?;
        fs::write(dir.0.join("docs/notes.md"), "# Notes\n\nmore\n")?;
        fs::write(dir.0.join("docs/new.md"), "# New\n")?;
        let mut app = app(&dir)?;

        let dirty: Vec<(String, State, bool)> = app
            .status()
            .entries()
            .iter()
            .map(|e| (e.path().display().to_string(), e.state(), e.is_staged()))
            .collect();
        assert_eq!(
            dirty,
            vec![
                ("README.md".to_owned(), State::Modified, false),
                ("docs/new.md".to_owned(), State::Untracked, false),
                ("docs/notes.md".to_owned(), State::Modified, false),
            ]
        );
        assert_eq!(
            app.status()
                .summary_under(Path::new("docs"))
                .map(|d| (d.state, d.added, d.removed)),
            Some((State::Untracked, 3, 0))
        );

        // `]g` walks README's two hunks, then crosses into the next dirty
        // files, then wraps.
        app.open(Path::new("README.md"));
        assert_eq!(app.view().diff_counts(), Some((4, 0)));
        app.hunk_next();
        assert_eq!(app.view().source_position().0, 3);
        app.hunk_next();
        assert_eq!(app.view().source_position().0, 7);
        app.hunk_next();
        assert_eq!(app.current_path(), Path::new("docs/new.md"));
        assert_eq!(app.view().source_position().0, 1);
        assert!(!app.view().line_staged(1), "untracked lines are unstaged");
        app.hunk_next();
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        assert_eq!(app.view().source_position().0, 3);
        app.hunk_next();
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.view().source_position().0, 3);
        assert_eq!(app.message(), Some("wrapped to first change"));

        // `[g` from README's first hunk lands on the last hunk of the last
        // dirty file.
        app.hunk_prev();
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        assert_eq!(app.view().source_position().0, 3);
        assert_eq!(app.message(), Some("wrapped to last change"));
        app.hunk_prev();
        assert_eq!(app.current_path(), Path::new("docs/new.md"));
        app.hunk_prev();
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert!(
            app.view().source_position().0 >= 6,
            "backwards lands on the last hunk"
        );

        // `]G` / `[G` step by file, always to the first hunk.
        app.dirty_next();
        assert_eq!(app.current_path(), Path::new("docs/new.md"));
        app.dirty_prev();
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.view().source_position().0, 3);
        app.dirty_prev();
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        assert_eq!(app.message(), Some("wrapped to last change"));

        // A clean file that is open steps into the next dirty one in
        // path order.
        app.open(Path::new("docs/guide.md"));
        app.hunk_next();
        assert_eq!(app.current_path(), Path::new("docs/new.md"));

        // Staging README marks its lines staged; the index event refreshes
        // both the bases and the dirty set.
        git::stage(
            &dir.0,
            &[
                ("README.md", "# Readme\n\nfirst\n\nhello\n\nlast\n"),
                ("docs/guide.md", "# Guide\n"),
                ("docs/notes.md", "# Notes\n"),
            ],
        )?;
        app.open(Path::new("README.md"));
        app.on_changes(vec![dir.0.join(".git/index")]);
        assert!(app.view().line_staged(3));
        assert!(app.view().line_staged(7));
        assert_eq!(
            app.view().diff_counts(),
            Some((4, 0)),
            "counts stay against HEAD"
        );
        assert_eq!(
            app.status()
                .get(Path::new("README.md"))
                .map(|e| (e.state(), e.is_staged())),
            Some((State::Modified, true))
        );

        // Committing everything empties the set.
        git::commit_and_stage(
            &dir.0,
            &[
                ("README.md", "# Readme\n\nfirst\n\nhello\n\nlast\n"),
                ("docs/guide.md", "# Guide\n"),
                ("docs/new.md", "# New\n"),
                ("docs/notes.md", "# Notes\n\nmore\n"),
            ],
        )?;
        app.on_changes(vec![dir.0.join(".git/HEAD")]);
        assert!(app.status().is_empty());
        assert_eq!(app.view().diff_counts(), Some((0, 0)));
        app.hunk_next();
        assert_eq!(app.message(), Some("nothing uncommitted"));
        Ok(())
    }

    #[test]
    fn workspace_changes_queue_newest_first_and_jump() -> anyhow::Result<()> {
        let dir = fixture("changes")?;
        let mut app = app(&dir)?;
        app.open(Path::new("README.md"));
        assert!(app.queue().is_empty());

        changed(&mut app, &dir, "docs/guide.md", "# Guide\n\nmore\n")?;
        changed(&mut app, &dir, "docs/notes.md", "# Notes\n\nnew\n")?;
        assert_eq!(app.queue().len(), 2);
        assert_eq!(
            app.queue().newest().map(|c| c.path.clone()),
            Some(PathBuf::from("docs/notes.md"))
        );
        assert!(app.has_change_under(Path::new("docs")));
        assert!(!app.has_change_under(Path::new("README.md")));
        assert_eq!(app.toasts().len(), 2);

        app.jump_newest();
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        assert_eq!(app.queue().len(), 1, "a visited change leaves the queue");
        app.jump_next();
        assert_eq!(app.current_path(), Path::new("docs/guide.md"));
        assert!(app.queue().is_empty());
        app.jump_next();
        assert_eq!(app.message(), Some("no changes"));

        // A change to the open file reloads it and lands on the first hunk.
        changed(
            &mut app,
            &dir,
            "docs/guide.md",
            "# Guide\n\nmore\n\nagain\n",
        )?;
        assert!(app.view().text().contains("again"));
        assert_eq!(app.queue().newest().map(|c| c.target.line()), Some(4));
        app.jump_newest();
        // The blank line 4 has no rendered row; the cursor lands on the
        // next one, as `]c` does.
        assert!((4..=5).contains(&app.view().source_position().0));
        Ok(())
    }

    #[test]
    fn watcher_events_refresh_the_listing_they_land_in() -> anyhow::Result<()> {
        use super::watch::Event;
        let dir = fixture("tree-events")?;
        let watch = WatchConfig {
            ignore: vec!["build/**".to_owned()],
            ..WatchConfig::default()
        };
        let mut app = app_with(
            &dir,
            Options {
                watch,
                ..Options::for_test(dir.0.clone())
            },
        )?;
        app.toggle_sidebar_focus();
        app.open_picker(PickerKind::Files);
        assert!(!picker_items(&app).iter().any(|p| p == "NEW.md"));
        app.close_popup();
        let has = |app: &App, path: &str| app.tree().is_some_and(|t| t.contains(Path::new(path)));

        // A created file lands in the root listing and the picker index.
        fs::write(dir.0.join("NEW.md"), "# New\n")?;
        app.on_events(vec![Event::Created(dir.0.join("NEW.md"))]);
        assert!(has(&app, "NEW.md"));
        assert_eq!(app.tree().map(Tree::cursor), Some(0), "cursor stays");
        app.open_picker(PickerKind::Files);
        assert!(picker_items(&app).iter().any(|p| p == "NEW.md"));
        app.close_popup();

        // If the platform says it lost events, a full rescan discovers a
        // creation whose individual event never arrived.
        fs::write(dir.0.join("MISSED.md"), "# Missed\n")?;
        app.on_events(vec![Event::Rescan]);
        assert!(has(&app, "MISSED.md"));
        app.open_picker(PickerKind::Files);
        assert!(picker_items(&app).iter().any(|p| p == "MISSED.md"));
        app.close_popup();

        // A collapsed directory is not re-read until it is expanded.
        fs::write(dir.0.join("docs/deep.md"), "# Deep\n")?;
        app.on_events(vec![Event::Created(dir.0.join("docs/deep.md"))]);
        assert!(!has(&app, "docs/deep.md"));
        app.with_tree_result(Tree::activate);
        assert!(has(&app, "docs/deep.md"));

        // A file written into a directory the tree has never listed —
        // an agent making a crate and filling it in one burst — brings
        // that directory into view; what is inside it waits for the
        // expansion.
        fs::create_dir_all(dir.0.join("crates/pipe/src"))?;
        fs::write(dir.0.join("crates/pipe/Cargo.toml"), "[package]\n")?;
        app.on_events(vec![Event::Created(dir.0.join("crates/pipe/Cargo.toml"))]);
        assert!(has(&app, "crates"));
        assert!(!has(&app, "crates/pipe"), "the new listing stays lazy");

        // A path `follow.ignore` hides never triggers a re-read.
        fs::create_dir_all(dir.0.join("build"))?;
        fs::write(dir.0.join("build/out"), "")?;
        app.on_events(vec![Event::Created(dir.0.join("build/out"))]);
        assert!(!has(&app, "build"));
        app.refresh_tree();
        assert!(has(&app, "build"));

        // A rename re-reads both listings.
        fs::rename(dir.0.join("NEW.md"), dir.0.join("docs/MOVED.md"))?;
        app.on_events(vec![Event::Renamed {
            from: dir.0.join("NEW.md"),
            to: dir.0.join("docs/MOVED.md"),
        }]);
        assert!(!has(&app, "NEW.md"));
        assert!(has(&app, "docs/MOVED.md"));
        Ok(())
    }

    #[test]
    fn followed_files_are_revealed_in_the_tree() -> anyhow::Result<()> {
        use fathomable_core::session::Request;
        let dir = fixture("follow-reveal")?;
        let mut app = app(&dir)?;
        app.show_sidebar();
        app.focus_pane(Focus::View);
        let has = |app: &App, path: &str| app.tree().is_some_and(|t| t.contains(Path::new(path)));
        assert!(!has(&app, "docs/guide.md"));

        // The view has focus: parents expand, the cursor stays put.
        app.handle_request(Request::Follow {
            paths: vec![PathBuf::from("docs/guide.md")],
        });
        assert!(has(&app, "docs/guide.md"));
        assert_eq!(app.tree().map(Tree::cursor), Some(0));
        assert!(!app.has_document());

        // The tree has focus: the cursor lands on it and shows it.
        app.with_tree(|tree, _| {
            tree.collapse();
            None
        });
        assert!(!has(&app, "docs/notes.md"));
        app.focus_pane(Focus::Sidebar);
        app.handle_request(Request::Follow {
            paths: vec![PathBuf::from("docs/notes.md")],
        });
        assert!(has(&app, "docs/notes.md"));
        assert_eq!(
            app.tree()
                .and_then(Tree::current)
                .map(|row| row.path().to_path_buf()),
            Some(PathBuf::from("docs/notes.md"))
        );
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        assert_eq!(app.focus(), Focus::Sidebar);
        Ok(())
    }

    #[test]
    fn new_and_removed_files_update_the_tree() -> anyhow::Result<()> {
        let dir = fixture("tree-watch")?;
        let mut app = app(&dir)?;
        app.toggle_sidebar_focus();
        let names = |app: &App| -> Vec<String> {
            app.tree()
                .map(|tree| {
                    tree.rows()
                        .iter()
                        .map(|row| row.name().to_owned())
                        .collect()
                })
                .unwrap_or_default()
        };
        assert!(!names(&app).contains(&"NEW.md".to_owned()));

        changed(&mut app, &dir, "NEW.md", "# New\n")?;
        assert!(
            names(&app).contains(&"NEW.md".to_owned()),
            "created file shows up"
        );

        let absolute = dir.0.join("NEW.md");
        fs::remove_file(&absolute)?;
        app.on_changes(vec![absolute]);
        assert!(
            !names(&app).contains(&"NEW.md".to_owned()),
            "removed file goes away"
        );
        Ok(())
    }

    #[test]
    fn ignore_rules_filter_hints_but_not_reloads() -> anyhow::Result<()> {
        let dir = fixture("source")?;
        let mut app = app(&dir)?;
        app.open(Path::new("README.md"));
        changed(&mut app, &dir, "README.md", "# Readme\n\nchanged\n")?;
        assert_eq!(app.queue().len(), 1, "an edit to the open file hints");
        assert!(
            app.view().text().contains("changed"),
            "and the open file reloads"
        );

        let watch = WatchConfig {
            ignore: vec!["docs/**".to_owned()],
            ..WatchConfig::default()
        };
        let jump = JumpConfig {
            toast: std::time::Duration::ZERO,
            ..JumpConfig::default()
        };
        let mut app = app_with(
            &dir,
            Options {
                jump,
                watch,
                ..Options::for_test(dir.0.clone())
            },
        )?;
        changed(&mut app, &dir, "docs/guide.md", "# Guide\n\n3\n")?;
        assert!(app.queue().is_empty(), "watch.ignore globs apply");
        changed(&mut app, &dir, "README.md", "# Readme\n\nx\n")?;
        assert_eq!(app.queue().len(), 1);
        assert!(app.toasts().is_empty(), "toast 0 disables toasts");

        app.command("auto");
        assert!(app.auto_jump());
        app.command("auto off");
        assert!(!app.auto_jump());
        app.command("status");
        assert!(matches!(app.popup(), Some(Popup::Status)));
        app.close_popup();
        app.command("nonsense");
        assert!(
            app.message()
                .is_some_and(|m| m.starts_with("not a command"))
        );
        Ok(())
    }

    /// The added lines of the last-seen diff view, or `None` when the
    /// view cannot show one.
    fn seen_diff_added(app: &mut App) -> Option<Vec<String>> {
        app.view_mut().toggle_seen_diff_view();
        if !app.view().diff_seen() {
            return None;
        }
        let added = app
            .view()
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .filter(|t| t.starts_with('+'))
            .collect();
        app.view_mut().toggle_seen_diff_view();
        Some(added)
    }

    #[test]
    fn seen_snapshots_feed_the_seen_diff_view() -> anyhow::Result<()> {
        let dir = fixture("seen")?;
        let seen_store = || fathomable_core::seen::Store::open(&dir.0.join(".seen-state"));
        let mut app = app_with(
            &dir,
            Options {
                seen: Some(seen_store()?),
                ..Options::for_test(dir.0.clone())
            },
        )?;
        app.open(Path::new("README.md"));
        assert_eq!(app.view().diff_counts(), None, "not in git: no gutter");
        assert_eq!(seen_diff_added(&mut app), None, "never seen: no seen diff");

        // Switching away snapshots the file; coming back, the seen diff
        // view shows what arrived, while the gutter stays git-only.
        app.open(Path::new("docs/guide.md"));
        fs::write(dir.0.join("README.md"), "# Readme\n\nhello\n\nworld\n")?;
        app.on_changes(vec![dir.0.join("README.md")]);
        assert_eq!(
            app.queue().newest().map(|c| c.target.line()),
            Some(4),
            "outside git an unopened file's target comes from its last-seen base"
        );
        app.open(Path::new("README.md"));
        assert_eq!(app.view().diff_counts(), None, "the gutter means git");
        assert_eq!(
            seen_diff_added(&mut app).as_deref(),
            Some(&["+".to_owned(), "+world".to_owned()][..])
        );

        // Idle long enough, the tick marks it seen and the next change is
        // measured from there.
        app.settle();
        assert!(
            app.queue().is_empty(),
            "target on screen settles the change"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        let idle = ViewerConfig {
            seen_idle: std::time::Duration::from_millis(1),
            ..ViewerConfig::default()
        };
        let mut app2 = app_with(
            &dir,
            Options {
                viewer: idle,
                seen: Some(seen_store()?),
                ..Options::for_test(dir.0.clone())
            },
        )?;
        app2.open(Path::new("README.md"));
        assert_eq!(seen_diff_added(&mut app2).map(|a| a.len()), Some(2));
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(app2.tick_in().is_some());
        app2.tick();
        fs::write(dir.0.join("README.md"), "# Readme\n\nhello\n\nworld\n\n!\n")?;
        app2.on_changes(vec![dir.0.join("README.md")]);
        assert_eq!(
            seen_diff_added(&mut app2).as_deref(),
            Some(&["+".to_owned(), "+!".to_owned()][..]),
            "base is the idle snapshot"
        );
        app2.on_quit();
        Ok(())
    }

    #[test]
    fn auto_jump_waits_for_quiet_and_guardrails() -> anyhow::Result<()> {
        let dir = fixture("auto")?;
        let jump = JumpConfig {
            auto: true,
            debounce: std::time::Duration::ZERO,
            ..JumpConfig::default()
        };
        let mut app = app_with(
            &dir,
            Options {
                jump,
                ..Options::for_test(dir.0.clone())
            },
        )?;
        app.open(Path::new("README.md"));
        changed(&mut app, &dir, "docs/notes.md", "# Notes\n\nnew\n")?;
        assert!(app.tick_in().is_some());
        app.tick();
        assert_eq!(
            app.current_path(),
            Path::new("README.md"),
            "the reader just opened a file: recent activity holds the jump"
        );

        // No activity in the welcome view: nothing open, so the jump goes.
        let jump = JumpConfig {
            auto: true,
            debounce: std::time::Duration::ZERO,
            ..JumpConfig::default()
        };
        let mut app = app_with(
            &dir,
            Options {
                jump,
                ..Options::for_test(dir.0.clone())
            },
        )?;
        changed(&mut app, &dir, "docs/notes.md", "# Notes\n\nnewer\n")?;
        app.tick();
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        assert!(app.queue().is_empty());

        let key = |c| {
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            )
        };
        super::input::keys::handle_key(&mut app, key(' '));
        super::input::keys::handle_key(&mut app, key('j'));
        assert_eq!(super::input::bindings::spell(app.prefix()), "Space j");
        super::input::keys::handle_key(&mut app, key('a'));
        assert!(!app.auto_jump());
        assert!(app.prefix().is_empty());
        Ok(())
    }

    #[test]
    fn agent_open_queues_a_settled_range_and_session_info_reports_state() -> anyhow::Result<()> {
        use fathomable_core::session::{Request, Response};

        let dir = fixture("agent")?;
        let mut app = app(&dir)?;
        let state = match app.handle_request(Request::SessionInfo) {
            Response::Session(_, state) => state,
            _ => None,
        };
        assert_eq!(
            state,
            Some(fathomable_core::session::FollowState { auto_jump: false })
        );
        let response = app.handle_request(Request::Open {
            path: PathBuf::from("README.md"),
            line: Some(1),
            end_line: Some(3),
        });
        assert_eq!(response, Response::Done);
        assert_eq!(app.queue().len(), 1);
        app.settle();
        assert!(app.queue().is_empty(), "the opened range is on screen");
        Ok(())
    }

    /// An agent's range is brought on screen, not selected: the reader
    /// is left in normal mode at its first line (ADR 0014, amended).
    #[test]
    fn agent_open_range_shows_without_selecting() -> anyhow::Result<()> {
        use fathomable_core::session::{Request, Response};

        let dir = fixture("agent-range")?;
        let body = "line\n".repeat(60);
        fs::write(dir.0.join("long.txt"), body)?;
        let mut app = app(&dir)?;
        app.resize(80, 12);
        let response = app.handle_request(Request::Open {
            path: PathBuf::from("long.txt"),
            line: Some(30),
            end_line: Some(36),
        });
        assert_eq!(response, Response::Done);
        assert_eq!(app.view().mode(), super::view::Mode::Normal);
        assert!(app.view().selection().is_none(), "nothing is selected");
        assert_eq!(app.view().cursor_source_line(), Some(30));
        assert!(app.view().line_on_screen(30));
        assert!(
            app.view().line_on_screen(36),
            "the end of the range is on screen"
        );

        // A range longer than the screen keeps its start visible.
        app.handle_request(Request::Open {
            path: PathBuf::from("long.txt"),
            line: Some(10),
            end_line: Some(60),
        });
        assert_eq!(app.view().cursor_source_line(), Some(10));
        assert!(app.view().line_on_screen(10));
        assert!(app.view().selection().is_none());
        Ok(())
    }

    #[test]
    fn binary_and_oversized_files_open_as_file_info() -> anyhow::Result<()> {
        use fathomable_core::config::ViewerConfig;

        let dir = fixture("binary")?;
        fs::write(dir.0.join("plugin.wasm"), b"\0asm\x01\0\0\0")?;
        fs::write(dir.0.join("big.log"), "x".repeat(3 * 1024 * 1024))?;
        let mut app = app_with(
            &dir,
            Options {
                viewer: ViewerConfig {
                    max_file_size_mib: 2,
                    ..ViewerConfig::default()
                },
                config_path: PathBuf::from("/etc/fathomable/config.kdl"),
                ..Options::for_test(dir.0.clone())
            },
        )?;

        app.open(Path::new("plugin.wasm"));
        assert_eq!(app.current_path(), Path::new("plugin.wasm"));
        let info = app
            .info()
            .ok_or_else(|| anyhow::anyhow!("a binary opens as file info"))?;
        let rows: Vec<(&str, &str)> = info
            .rows
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(rows[0], ("format", "WebAssembly module"));
        assert_eq!(rows[1], ("size", "8 B"));
        assert_eq!(rows[2], ("mode", "regular file"));
        assert_eq!(rows.last(), Some(&("git", "no repository")));
        assert_eq!(app.view().text(), "");
        app.start_comment();
        assert_eq!(app.message(), Some("cannot annotate a binary file"));
        assert!(app.popup().is_none());

        app.open(Path::new("big.log"));
        let info = app
            .info()
            .ok_or_else(|| anyhow::anyhow!("an oversized file opens as file info"))?;
        assert_eq!(info.rows[0].1, "text, too large to view");
        assert_eq!(
            info.notice,
            [
                "Too large to view: 3 MiB, limit is 2 MiB.",
                "Raise it with `viewer { max-file-size-mib 4 }` in /etc/fathomable/config.kdl",
            ]
        );
        app.start_comment();
        assert_eq!(app.message(), Some("cannot annotate a file this large"));

        // Text files are unaffected and the jumplist spans both kinds.
        app.record_jump_from_here();
        app.open(Path::new("README.md"));
        assert!(app.info().is_none());
        app.jump_back();
        assert_eq!(app.current_path(), Path::new("big.log"));
        assert!(app.info().is_some());
        Ok(())
    }
}
