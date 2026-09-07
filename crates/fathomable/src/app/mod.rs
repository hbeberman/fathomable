// @okf-doc: /decisions/0012-workspace-mode.md
//! The workspace app: open documents, sidebar, popups, and the state every
//! concept module hangs off.
//!
//! [`App`] is plain state so the viewer's behaviour is tested without a
//! terminal. The modules are grouped by concept (ADR 0048): `threads`
//! holds the thread cursor, the panes, the list, and the store operations;
//! `draw` renders; `input` binds and dispatches keys and the mouse; `jump`
//! is auto-jump; `agents` wakes subscribers; `sidebar` is the column and
//! `files_pane` its upper pane; `view`, `watch`, `socket`, `commands`, and
//! `clipboard` are what their names say; and
//! [`run`] owns the terminal, the file watcher, and the viewer socket.

pub(crate) mod agents;
mod checkpoints;
mod clipboard;
mod commands;
mod diff;
mod draw;
mod file_index;
mod files_pane;
mod files_shown;
mod goto_file;
pub(crate) mod input;
mod jump;
mod jumplist;
pub(crate) mod run;
mod sidebar;
mod socket;
mod status_walk;
#[cfg(test)]
pub(crate) mod testing;
pub(crate) mod threads;
mod view;
mod watch;
mod window;

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::threads::list::ReviewList;
use fathomable_core::annotations::{self, Reach, Store, ThreadId};
use fathomable_core::config::{
    AgentsConfig, DiffConfig, JumpConfig, MarkdownConfig, SidebarConfig, ThreadsConfig, UserConfig,
    ViewerConfig, WatchConfig,
};
use fathomable_core::content::Policy;
use fathomable_core::diff::Diff;
use fathomable_core::follow::{Change, Ignore, Queue, Target};
use fathomable_core::highlight::{Highlighter, language_hint};
use fathomable_core::picker::{Match, Picker};
use fathomable_core::seen;
use fathomable_core::session::{Record, Request, Response};
use fathomable_core::status::Status;
use fathomable_core::tree::Tree;
use fathomable_core::workspace::{EntryKind, Filter, Workspace, WorkspaceError, is_rules_file};
use fathomable_core::{Document, XdgDirs};
use input::bindings::Chord;

pub(crate) use threads::cursor::ThreadCursor;
pub(crate) use threads::{Compose, Mark};
use view::{HunkStep, Syntax, View};
use watch::{Fingerprint, is_git_metadata};

/// How long to wait after a change notification before re-reading, so an
/// editor's write-then-rename lands as one reload.
/// Toasts visible at once.
pub(crate) const MAX_TOASTS: usize = 3;

/// A transient one-line notice about a change (ADR 0015).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Toast {
    text: String,
    until: Instant,
}

impl Toast {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

/// Fewest text columns a drag leaves the view.
const TEXT_MIN_WIDTH: usize = 20;

/// Rows kept visible above and below the tree cursor.
const TREE_SCROLLOFF: usize = 2;

/// Which pane receives keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    View,
    /// The sidebar's files pane.
    Tree,
    /// The review list (ADR 0025, ADR 0049).
    Review,
    /// The sidebar's threads pane (ADR 0027, ADR 0049).
    ThreadsPane,
}

/// A pane border the mouse is dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Border {
    /// The rule between the sidebar and the text.
    Sidebar,
    /// The rule along the top of the threads pane (ADR 0027).
    ThreadsPane,
}

/// What the file picker lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PickerKind {
    /// Workspace files, ignore-filtered.
    Files,
    /// Workspace files including ignored ones.
    AllFiles,
    /// Documents opened this session, most recent first.
    Recent,
    /// Subscribed agents to wake with `Space a w` (ADR 0040).
    Wake,
    /// The base side of the diff (ADR 0060).
    DiffBase,
    /// The target side of the diff (ADR 0060).
    DiffTarget,
}

/// The open picker popup.
#[derive(Debug)]
pub(crate) struct PickerState {
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

    pub(crate) fn kind(&self) -> PickerKind {
        self.kind
    }

    pub(crate) fn input(&self) -> &str {
        &self.input
    }

    pub(crate) fn matches(&self) -> &[Match] {
        &self.matches
    }

    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    pub(crate) fn item(&self, m: &Match) -> &str {
        &self.picker.items()[m.index()]
    }

    /// Number of candidates before filtering.
    pub(crate) fn total(&self) -> usize {
        self.picker.items().len()
    }

    fn requery(&mut self) {
        self.matches = self.picker.query(&self.input);
        self.selected = 0;
    }
}

/// A popup layered over the panes.
#[derive(Debug)]
pub(crate) enum Popup {
    /// Every key binding.
    Help,
    /// A file, recent-document, or thread picker.
    Picker(PickerState),
    /// The draft being written in the text (ADR 0013, 0054): a popup
    /// only in that it takes the keys.
    Compose(Compose),
    /// The `:status` overlay (ADR 0021).
    Status,
    /// The context menu a right-click opened (ADR 0050).
    Menu(input::menu::Menu),
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
pub(crate) struct App {
    workspace: Workspace,
    docs: Vec<Doc>,
    current: Option<usize>,
    /// Documents by index, most recently shown first (`Space F r`).
    recent: Vec<usize>,
    jumplist: jumplist::Jumplist,
    /// Where a search started, recorded on the jumplist when it lands
    /// somewhere else (ADR 0049).
    search_origin: Option<jumplist::Position>,
    welcome: View,
    tree: Option<Tree>,
    /// The sidebar's panes, scope, split, and sizes (ADR 0049, ADR 0057).
    sidebar: sidebar::Sidebar,
    /// Whether stubs are drawn, and for resolved threads (ADR 0049).
    stubs: threads::stubs::StubState,
    /// The threads expanded in place this session (ADR 0049).
    expanded: HashSet<ThreadId>,
    /// A `c` cycle in progress: the thread it started on and the threads
    /// it walks (ADR 0049).
    cycle: Option<(ThreadId, Vec<ThreadId>)>,
    /// What the review shows, shared by the list and the threads pane.
    review: threads::list::ReviewState,
    tree_scroll: usize,
    /// Tree width once dragged; the default follows the terminal.
    sidebar_cols: Option<usize>,
    /// The thread and message the thread surfaces show; authoritative
    /// while the pane or the list is open, or the text cursor rests
    /// where `thread_cursor_anchor` says it was set (ADR 0046).
    thread_cursor: ThreadCursor,
    /// `(document, row)` of the text cursor when the thread cursor was
    /// last set.
    thread_cursor_anchor: Option<(Option<usize>, usize)>,
    /// The review list shown in place of the document (ADR 0025).
    review_list: ReviewList,
    /// File-threads pane height once dragged; the default follows its
    /// The border a mouse drag is moving.
    drag: Option<Border>,
    /// The cell the pointer was last seen at, for hover (ADR 0050).
    pointer: Option<(usize, usize)>,
    /// The last left press in the text, for click counts (ADR 0050).
    press: Option<input::mouse::Press>,
    focus: Focus,
    popup: Option<Popup>,
    /// The `Space f` files (ADR 0028), patched as paths come and go.
    file_index: file_index::FileIndex,
    /// The same with ignored files, for `I`.
    all_index: file_index::FileIndex,
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
    /// How the person at the viewer is named (ADR 0058).
    user: UserConfig,
    /// Who watches which thread, refreshed with the store (ADR 0040).
    watchers: Vec<(ThreadId, String)>,
    /// The config file the over-limit notice names (ADR 0026).
    config_path: PathBuf,
    auto: bool,
    ignore: Ignore,
    queue: Queue,
    toasts: Vec<Toast>,
    seen: Option<seen::Store>,
    /// The reader's checkpoints (ADR 0049).
    checkpoints: Option<fathomable_core::checkpoints::Store>,
    /// What each item of an open side picker names (ADR 0060).
    diff_choices: Vec<(String, diff::Side)>,
    /// The target `Space d g` fixes for the next base choice.
    diff_target_next: Option<diff::Side>,
    /// How diffs are compared this session: the config's start, then
    /// `Space d w` (ADR 0060).
    compare: fathomable_core::diff::Compare,
    /// When the queue last changed, for the auto-jump debounce.
    last_change: Option<Instant>,
    /// Whether the recursive workspace watch is in place.
    watching_root: bool,
    /// Whether the dirty set missed a refresh, so the next one must
    /// walk the whole tree rather than build on it.
    status_stale: bool,
    /// The full walks of the dirty set, off the loop (ADR 0017).
    walks: status_walk::Walks,
    /// Every uncommitted path (ADR 0017), refreshed on git and file events.
    status: Status,
}

impl App {
    /// Start with no document open, for a terminal of `width` by `height`.
    ///
    /// Threads on files edited while Fathomable was closed are followed
    /// through their last-seen snapshots before anything opens (ADR 0020).
    pub(crate) fn new(workspace: Workspace, width: usize, height: usize, options: Options) -> Self {
        let Options {
            record,
            dirs,
            store,
            jump,
            watch,
            seen,
            checkpoints,
            highlighter,
            markdown,
            viewer,
            sidebar,
            threads,
            diff,
            agents,
            user,
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
            sidebar: sidebar::Sidebar::new(sidebar),
            stubs: threads::stubs::StubState::from_config(&threads),
            expanded: HashSet::new(),
            cycle: None,
            review: threads::list::ReviewState::default(),
            tree_scroll: 0,
            sidebar_cols: None,
            thread_cursor: ThreadCursor::default(),
            thread_cursor_anchor: None,
            review_list: ReviewList::default(),
            drag: None,
            pointer: None,
            press: None,
            focus: Focus::View,
            popup: None,
            file_index: file_index::FileIndex::new(Filter::Visible),
            all_index: file_index::FileIndex::new(Filter::All),
            message: None,
            prefix: Vec::new(),
            pending_delete: None,
            width,
            height,
            viewer_id: record.id().to_string(),
            store,
            reach: Reach::everything(),
            record,
            dirs,
            auto: jump.auto,
            jump,
            highlighter,
            markdown,
            viewer,
            agents,
            user,
            watchers: Vec::new(),
            config_path,
            ignore,
            queue: Queue::default(),
            toasts: Vec::new(),
            seen,
            checkpoints,
            diff_choices: Vec::new(),
            diff_target_next: None,
            compare: diff.compare(),
            last_change: None,
            watching_root: false,
            status_stale: false,
            walks: status_walk::Walks::new(),
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
    /// expanded threads follow.
    pub(crate) fn reload_store(&mut self) {
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
    pub(crate) fn store_path(&self) -> Option<&Path> {
        self.store.as_ref().map(Store::path)
    }

    /// `:name`: rename this viewer for agents; empty clears the name.
    pub(crate) fn set_name(&mut self, name: Option<&str>) {
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
    /// The code highlighter shared by every view and the expanded threads.
    /// The name the user's messages carry (ADR 0058).
    pub(crate) fn user_name(&self) -> &str {
        &self.user.name
    }

    pub(crate) fn highlighter(&self) -> &Highlighter {
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
    /// Whether the recursive workspace watch is in place; when it is
    /// not, the user is told, since the tree, the follow queue, and the
    /// dirty set all go quiet without it (ADR 0015).
    pub(crate) fn set_watching_root(&mut self, watching: bool) {
        self.watching_root = watching;
        if !watching {
            self.notice(
                "cannot watch the workspace; following the open file only (--doctor counts the directories)",
            );
        }
    }

    // ----- follow mode (ADR 0015) -----

    /// Whether auto-jump is on.
    pub(crate) fn auto_jump(&self) -> bool {
        self.auto
    }

    /// Changed files, newest first.
    pub(crate) fn queue(&self) -> &Queue {
        &self.queue
    }

    /// Live toasts, oldest first.
    pub(crate) fn toasts(&self) -> &[Toast] {
        &self.toasts
    }

    /// Whether a queued change sits at or under the root-relative `path`.
    pub(crate) fn has_change_under(&self, path: &Path) -> bool {
        self.queue.iter().any(|c| c.path.starts_with(path))
    }

    /// Toggle auto-jump (`Space j a`, `:auto`).
    pub(crate) fn toggle_auto_jump(&mut self) {
        self.auto = !self.auto;
        self.notice(if self.auto {
            "auto-jump on"
        } else {
            "auto-jump off"
        });
    }

    /// Set auto-jump (`:auto on` / `:auto off`).
    pub(crate) fn set_auto_jump(&mut self, on: bool) {
        self.auto = on;
        self.notice(if on { "auto-jump on" } else { "auto-jump off" });
    }

    /// `Space j j`: open the newest change.
    pub(crate) fn jump_newest(&mut self) {
        match self.queue.newest().cloned() {
            Some(change) => self.jump_to(&change),
            None => self.notice("no changes"),
        }
    }

    /// `]f`: the next older change after the current file, wrapping.
    pub(crate) fn jump_next(&mut self) {
        let current = self.current.map(|i| self.docs[i].relative.clone());
        match self.queue.after(current.as_deref()).cloned() {
            Some(change) => self.jump_to(&change),
            None => self.notice("no changes"),
        }
    }

    /// `[f`: the next newer change before the current file, wrapping.
    pub(crate) fn jump_prev(&mut self) {
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
    pub(crate) fn on_changes(&mut self, paths: Vec<PathBuf>) {
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
        self.settle_status();
    }

    /// What one settled watcher batch did (ADR 0028). Loaded documents
    /// reload, follow their rename, or keep their content under a
    /// `deleted` banner; changes that pass the source and ignore rules
    /// join the queue; the directories whose listings changed are
    /// re-read in the tree. A platform event-loss notice reconciles the
    /// whole remembered workspace from disk.
    pub(crate) fn on_events(&mut self, events: Vec<watch::Event>) {
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
        // Ignore rules first: what follows asks them about every path.
        let rules_changed = self.reload_rules_named_in(&events);
        let banner_before = self.banner().is_some();
        let mut git_changed = false;
        // Root-relative paths the batch named, for the dirty set.
        let mut changed: Vec<PathBuf> = Vec::new();
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
            if let watch::Event::Renamed { from, .. } = &event
                && let Ok(from) = from.strip_prefix(&root)
            {
                changed.push(from.to_path_buf());
            }
            changed.push(relative.clone());
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
        if rules_changed {
            // What the index shows follows the rules: walk it again.
            self.file_index.clear();
            self.all_index.clear();
        }
        if git_changed || rules_changed {
            self.refresh_status();
        } else if !changed.is_empty() {
            self.refresh_status_for(&changed);
        }
        // The banner row takes a text row, so the view re-fits when it
        // comes or goes.
        if banner_before != self.banner().is_some() {
            self.relayout();
        }
        if !dirs.is_empty() {
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
        self.reload_rules();
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
        self.file_index.clear();
        self.all_index.clear();
        self.with_tree_result(|tree, workspace| tree.refresh(workspace).map(|()| None));
    }

    /// Re-read the ignore rules when `events` name an ignore or attribute
    /// file (ADR 0012); whether they did. A failure is reported and keeps
    /// the rules in use.
    fn reload_rules_named_in(&mut self, events: &[watch::Event]) -> bool {
        let root = self.workspace.root();
        let named = events.iter().any(|event| {
            event
                .path()
                .and_then(|path| path.strip_prefix(root).ok())
                .is_some_and(is_rules_file)
        });
        if named {
            self.reload_rules();
        }
        named
    }

    /// Re-read the ignore rules; a failure is reported and keeps the
    /// rules in use.
    fn reload_rules(&mut self) {
        if let Err(error) = self.workspace.reload_rules() {
            self.notice(format!("ignore rules: {error}"));
        }
    }

    /// Note that the listing holding root-relative `path` changed, unless
    /// the tree would hide the path anyway (ADR 0028): build output
    /// churning under `target/` costs nothing here.
    fn note_dir(&mut self, path: &Path, dirs: &mut Vec<PathBuf>) {
        if self.ignore.is_ignored(path) {
            return;
        }
        let kind = if self.workspace.root().join(path).is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        if self.workspace.is_ignored(path, kind) {
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
        self.file_index.removed(relative);
        self.all_index.removed(relative);
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
        self.file_index.removed(from);
        self.all_index.removed(from);
        self.file_index.seen(&mut self.workspace, to);
        self.all_index.seen(&mut self.workspace, to);
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
    /// The largest file the viewer reads (`viewer.max-file-size-mib`).
    pub(crate) fn max_file_bytes(&self) -> u64 {
        self.viewer.max_file_bytes()
    }

    pub(crate) fn last_seen_fingerprint(&self, path: &Path) -> Option<Fingerprint> {
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
    pub(crate) fn status(&self) -> &Status {
        &self.status
    }

    /// Walk the whole dirty set again, on a thread of its own: at start,
    /// after a `.git` or ignore rules change, and after lost events. The
    /// set in hand stays until the walk lands (`on_walked`); a failure
    /// to start is reported and leaves it to be walked next time.
    fn refresh_status(&mut self) {
        if !self.workspace.is_git() {
            self.take_status(Ok(Status::default()));
            return;
        }
        if let Err(error) = self.walks.start(self.workspace.root().to_path_buf()) {
            self.status_stale = true;
            self.notice(format!("git status: cannot start the walk: {error}"));
        }
    }

    /// Re-read the dirty set for the root-relative paths a settled batch
    /// named (ADR 0017): the rest is kept, so a burst of writes costs
    /// the paths it touched, never a walk of the tree. A walk in flight
    /// examines them again when it lands.
    fn refresh_status_for(&mut self, changed: &[PathBuf]) {
        self.walks.note_changed(changed);
        if self.status_stale {
            if !self.walks.in_flight() {
                self.refresh_status();
            }
            return;
        }
        let result = self.workspace.status_after(&self.status, changed);
        self.take_status(result);
    }

    /// The next full walk's result, when it lands.
    pub(crate) async fn next_walk(&mut self) -> status_walk::Walked {
        self.walks.next().await
    }

    /// Take a full walk's result: the paths that changed while it ran
    /// are examined again on it, so nothing written meanwhile is lost.
    /// An older walk's result, superseded by a newer one, is dropped.
    pub(crate) fn on_walked(&mut self, walked: status_walk::Walked) {
        let Some((result, changed)) = self.walks.accept(walked) else {
            return;
        };
        let result = result.and_then(|status| {
            if changed.is_empty() {
                Ok(status)
            } else {
                self.workspace.status_after(&status, &changed)
            }
        });
        self.take_status(result);
    }

    /// Wait for every walk in flight and take its result, so a test sees
    /// the set the loop would show once the walk lands.
    #[cfg(test)]
    pub(crate) fn settle_status(&mut self) {
        while self.walks.in_flight() {
            let Some(walked) = self.walks.blocking_next() else {
                return;
            };
            self.on_walked(walked);
        }
    }

    fn take_status(&mut self, result: Result<Status, WorkspaceError>) {
        match result {
            Ok(status) => {
                if status != self.status {
                    tracing::info!(dirty = status.len(), "dirty set changed");
                }
                self.status = status;
                self.status_stale = false;
                self.sift_tree();
            }
            Err(error) => {
                self.status_stale = true;
                self.notice(format!("git status: {error}"));
            }
        }
    }

    /// `]g`: the next hunk in this file, or the first hunk of the next
    /// uncommitted file when this one's run out, wrapping.
    pub(crate) fn hunk_next(&mut self) {
        self.step_hunk(true);
    }

    /// `[g`: the previous hunk, crossing into the last hunk of the
    /// previous uncommitted file.
    pub(crate) fn hunk_prev(&mut self) {
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
    pub(crate) fn dirty_next(&mut self) {
        self.step_dirty(true, false);
    }

    /// `[G`: the previous uncommitted file, at its first hunk.
    pub(crate) fn dirty_prev(&mut self) {
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
            self.notice(if self.walks.in_flight() {
                "git status is still walking the tree"
            } else {
                "nothing uncommitted"
            });
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
        // A new file is one `Space f` away (ADR 0028).
        self.file_index.seen(&mut self.workspace, relative);
        self.all_index.seen(&mut self.workspace, relative);
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
    pub(crate) fn settle(&mut self) {
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
    pub(crate) fn tick(&mut self) {
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
    pub(crate) fn tick_in(&self) -> Option<Duration> {
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
    pub(crate) fn on_quit(&mut self) {
        if let Some(index) = self.current {
            self.mark_seen(index);
        }
    }

    pub(crate) fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub(crate) fn focus(&self) -> Focus {
        self.focus
    }

    /// A click lands in a pane: it takes the keys, when it is on screen.
    pub(crate) fn focus_pane(&mut self, focus: Focus) {
        let present = match focus {
            Focus::View => true,
            Focus::Tree => self.tree().is_some(),
            Focus::Review => self.review_list.is_open(),
            Focus::ThreadsPane => self.threads_pane_height() > 0,
        };
        if present {
            self.focus = focus;
        }
    }

    /// Whether a document is open, rather than the welcome screen.
    pub(crate) fn has_document(&self) -> bool {
        self.current.is_some()
    }

    /// The border a drag is moving, while the button is down.
    pub(crate) fn dragging(&self) -> Option<Border> {
        self.drag
    }

    /// The mouse went down on a border.
    pub(crate) fn begin_drag(&mut self, border: Border) {
        self.drag = Some(border);
    }

    /// The mouse moved with a border held: the tree's divider follows the
    /// column, the threads pane's rule follows the row.
    pub(crate) fn drag_to(&mut self, column: usize, row: usize) {
        match self.drag {
            Some(Border::Sidebar) => self.sidebar_cols = Some(column + 1),
            Some(Border::ThreadsPane) => self.drag_threads_pane_to(row),
            None => return,
        }
        self.relayout();
    }

    pub(crate) fn end_drag(&mut self) {
        self.drag = None;
    }

    /// The cell the pointer was last seen at (ADR 0050).
    #[must_use]
    pub(crate) fn pointer(&self) -> Option<(usize, usize)> {
        self.pointer
    }

    /// The terminal's size as the app last heard it.
    #[must_use]
    pub(crate) fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub(crate) fn popup(&self) -> Option<&Popup> {
        self.popup.as_ref()
    }

    pub(crate) fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref().filter(|_| self.sidebar.tree)
    }

    pub(crate) fn tree_scroll(&self) -> usize {
        self.tree_scroll
    }

    /// The notice on the status line: the answer to the reader's last key,
    /// from the app or from the view. A toast, by contrast, reports what
    /// happened without the reader (ADR 0010).
    pub(crate) fn message(&self) -> Option<&str> {
        self.message.as_deref().or_else(|| self.view().message())
    }

    /// The keys typed so far of a binding that is not complete.
    pub(crate) fn prefix(&self) -> &[Chord] {
        &self.prefix
    }

    /// Remember `typed` as the start of a longer binding.
    pub(crate) fn set_prefix(&mut self, typed: Vec<Chord>) {
        self.prefix = typed;
    }

    /// Take the pending keys, leaving none.
    pub(crate) fn take_prefix(&mut self) -> Vec<Chord> {
        std::mem::take(&mut self.prefix)
    }

    /// The visible view: the current document's or the welcome text.
    pub(crate) fn view(&self) -> &View {
        self.current
            .and_then(|i| self.docs.get(i))
            .map_or(&self.welcome, |doc| &doc.view)
    }

    pub(crate) fn view_mut(&mut self) -> &mut View {
        match self.current.and_then(|i| self.docs.get_mut(i)) {
            Some(doc) => &mut doc.view,
            None => &mut self.welcome,
        }
    }

    /// Root-relative path of the current document, empty when none is open.
    pub(crate) fn current_path(&self) -> &Path {
        self.current
            .and_then(|i| self.docs.get(i))
            .map_or(Path::new(""), |doc| &doc.relative)
    }

    /// Absolute path of the current document, for the watcher.
    pub(crate) fn current_abs_path(&self) -> Option<&Path> {
        self.current
            .and_then(|i| self.docs.get(i))
            .map(|doc| doc.document.path())
    }

    /// Whether the current document's file is gone from disk (ADR 0028).
    pub(crate) fn deleted(&self) -> bool {
        self.current
            .and_then(|i| self.docs.get(i))
            .is_some_and(|doc| doc.deleted.is_some())
    }

    /// The banner row over the text: `deleted` while the current file is
    /// gone and its last content is still shown (ADR 0028).
    pub(crate) fn banner(&self) -> Option<&'static str> {
        self.current
            .and_then(|i| self.docs.get(i))
            .filter(|doc| doc.deleted == Some(Deleted::Banner) && !self.review_list.is_open())
            .map(|_| "deleted")
    }

    /// Whether the current document is shown as the file-info pane
    /// because it was deleted and shown again while gone (ADR 0028).
    pub(super) fn deleted_info(&self) -> bool {
        self.current
            .and_then(|i| self.docs.get(i))
            .is_some_and(|doc| doc.deleted == Some(Deleted::Info))
    }

    /// Rows available to panes once the status line is taken.
    pub(crate) fn pane_rows(&self) -> usize {
        self.height.saturating_sub(1).max(1)
    }

    /// Bracketed paste: into the draft, else nothing to paste into.
    pub(crate) fn paste(&mut self, text: &str) {
        if matches!(self.popup, Some(Popup::Compose(_))) {
            self.compose_insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
        }
    }

    /// Rows left to the text once the banner and the diff's header and
    /// strip are taken. The text's key bar takes none: it replaces the
    /// bottom text row while it has something to say (ADR 0067).
    pub(crate) fn text_rows(&self) -> usize {
        self.pane_rows()
            .saturating_sub(usize::from(self.banner().is_some()))
            .saturating_sub(self.diff_chrome_rows())
            .max(1)
    }

    /// Whether the text's key bar replaces the bottom text row (ADR
    /// 0067): the column shows a document, and a draft is open, a thread
    /// is under the cursor, or the file has a thread to fold. The review
    /// list and the file-info pane have no bar of this kind.
    pub(crate) fn text_bar_shown(&self) -> bool {
        self.has_document()
            && !self.review_list().is_open()
            && self.info().is_none()
            && (self.draft().is_some()
                || self
                    .thread_cursor()
                    .thread()
                    .is_some_and(|id| self.threads_at_cursor().contains(id))
                || !self.stubs().is_empty())
    }

    /// The screen row the text's key bar replaces: the bottom text row.
    pub(crate) fn text_bar_row(&self) -> usize {
        self.text_top() + self.text_rows() - 1
    }

    /// Rows over the text: the banner and the diff's header.
    pub(crate) fn text_top(&self) -> usize {
        usize::from(self.banner().is_some()) + usize::from(self.diff_chrome_rows() > 0)
    }

    pub(crate) fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.relayout();
        // Message and draft rows wrap at the new width (ADR 0049, 0054).
        self.place_stub_rows();
    }

    fn relayout(&mut self) {
        let rows = self.text_rows();
        let sidebar = self.sidebar_width();
        let text_width = self
            .width
            .saturating_sub(sidebar)
            .saturating_sub(crate::app::draw::gutter_width(self.view()))
            .max(1);
        self.view_mut().resize(text_width, rows);
        self.scroll_tree();
    }

    pub(crate) fn clear_message(&mut self) {
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
    pub(crate) fn open(&mut self, path: &Path) {
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
                    let mut view = View::with_syntax(
                        document.text().unwrap_or_default().to_owned(),
                        1,
                        1,
                        self.syntax_for(&relative),
                    );
                    view.set_compare(self.compare);
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
    pub(crate) fn handle_request(&mut self, request: Request) -> Response {
        match request {
            Request::Open {
                path,
                line,
                end_line,
            } => self.open_for_agent(&path, line, end_line),
            Request::ThreadsList { since, path } => match &self.store {
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
                Ok(thread) => Response::Threads(vec![thread]),
                Err(error) => Response::Error(error),
            },
            Request::ThreadStart {
                path,
                range,
                author,
                body,
            } => match self.agent_start(&path, range, author, body) {
                Ok(thread) => Response::Threads(vec![thread]),
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
        self.review_list.close();
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
    pub(crate) fn jump_back(&mut self) {
        let Some(here) = self.position() else {
            return;
        };
        match self.jumplist.back(here).cloned() {
            Some(target) => self.go_to_position(&target),
            None => self.notice("at oldest position"),
        }
    }

    /// `Alt-Right`: the next position in the jumplist.
    pub(crate) fn jump_forward(&mut self) {
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

    /// What a start shows: the file named, else the sidebar with both panes
    /// and the tree focused (ADR 0012, ADR 0049).
    pub(crate) fn start_on(&mut self, open: Option<&Path>) {
        if let Some(path) = open {
            self.open(path);
        } else {
            self.show_tree();
            self.show_threads_pane();
        }
    }

    /// Show and focus the tree, as a workspace start does (ADR 0012).
    pub(crate) fn show_tree(&mut self) {
        if !self.sidebar.tree {
            self.toggle_tree_focus();
        }
    }

    /// Open and focus the files pane, or hand focus back: the tree's
    /// `Esc` and the text's `h` at column 0 (ADR 0056 unbound `Space e`).
    pub(crate) fn toggle_tree_focus(&mut self) {
        if !self.sidebar.tree {
            if !self.ensure_tree() {
                return;
            }
            self.sidebar.tree = true;
            self.reveal_current();
            self.focus = Focus::Tree;
        } else if self.focus == Focus::Tree {
            self.focus = Focus::View;
        } else {
            self.reveal_current();
            self.focus = Focus::Tree;
        }
        self.relayout();
    }

    /// `Space p f`: hide the files pane, or show it again without taking
    /// the keys; the threads pane keeps the sidebar either way.
    pub(crate) fn toggle_tree_shown(&mut self) {
        if self.sidebar.tree {
            self.sidebar.tree = false;
            if self.focus == Focus::Tree {
                self.focus = Focus::View;
            }
        } else {
            if !self.ensure_tree() {
                return;
            }
            self.sidebar.tree = true;
            self.reveal_current();
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

    // ----- popups -----

    pub(crate) fn open_help(&mut self) {
        self.popup = Some(Popup::Help);
    }

    /// `:status`: the overlay of session facts (ADR 0021).
    pub(crate) fn open_status(&mut self) {
        self.popup = Some(Popup::Status);
    }

    pub(crate) fn close_popup(&mut self) {
        self.popup = None;
    }

    pub(crate) fn open_picker(&mut self, kind: PickerKind) {
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
            PickerKind::DiffBase | PickerKind::DiffTarget => self.diff_choices(),
        };
        tracing::info!(?kind, items = items.len(), "picker opened");
        self.popup = Some(Popup::Picker(PickerState::new(kind, items)));
    }

    fn index(&mut self, filter: Filter) -> Vec<String> {
        match filter {
            Filter::All => self.all_index.files(&mut self.workspace),
            Filter::Visible => self.file_index.files(&mut self.workspace),
        }
    }

    fn picker_mut(&mut self) -> Option<&mut PickerState> {
        match self.popup.as_mut() {
            Some(Popup::Picker(state)) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn picker_char(&mut self, ch: char) {
        if let Some(picker) = self.picker_mut() {
            picker.input.push(ch);
            picker.requery();
        }
    }

    pub(crate) fn picker_backspace(&mut self) {
        if let Some(picker) = self.picker_mut() {
            picker.input.pop();
            picker.requery();
        }
    }

    pub(crate) fn picker_move(&mut self, delta: isize) {
        if let Some(picker) = self.picker_mut() {
            let last = picker.matches.len().saturating_sub(1);
            picker.selected = picker.selected.saturating_add_signed(delta).min(last);
        }
    }

    /// Enter in the picker: open the file, or show the thread.
    pub(crate) fn picker_confirm(&mut self) {
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
            Some((kind @ (PickerKind::DiffBase | PickerKind::DiffTarget), item)) => {
                self.choose_diff_side(kind, &item);
            }
            None => {}
        }
    }
}

/// Everything [`App::new`] needs beyond the workspace and terminal size.
#[derive(Debug)]
pub(crate) struct Options {
    /// This viewer's record, already written to the viewers directory.
    pub(crate) record: Record,
    /// Where records and state live, for `:name` to rewrite the record.
    pub(crate) dirs: XdgDirs,
    /// The workspace's thread store, or `None` when it could not be opened.
    pub(crate) store: Option<Store>,
    /// Auto-jump settings (ADR 0015).
    pub(crate) jump: JumpConfig,
    /// File-watcher settings (ADR 0015).
    pub(crate) watch: WatchConfig,
    /// The last-seen snapshot store, or `None` when it could not be opened.
    pub(crate) seen: Option<seen::Store>,
    /// The checkpoint store (ADR 0049), or `None` when it could not be opened.
    pub(crate) checkpoints: Option<fathomable_core::checkpoints::Store>,
    /// Code highlighting for fences and source files (ADR 0016).
    pub(crate) highlighter: Arc<Highlighter>,
    /// Which files render as Markdown (ADR 0016).
    pub(crate) markdown: MarkdownConfig,
    /// How files are read (ADR 0026).
    pub(crate) viewer: ViewerConfig,
    /// The sidebar's width and split (ADR 0049).
    pub(crate) sidebar: SidebarConfig,
    /// How threads show in the text (ADR 0049).
    pub(crate) threads: ThreadsConfig,
    /// How diffs are compared and listed (ADR 0060).
    pub(crate) diff: DiffConfig,
    /// Subscriptions and the wake command (ADR 0040).
    pub(crate) agents: AgentsConfig,
    /// How the person at the viewer is named (ADR 0058).
    pub(crate) user: UserConfig,
    /// The config file in use, for the over-limit notice (ADR 0026).
    pub(crate) config_path: PathBuf,
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
            checkpoints: None,
            highlighter: Arc::new(Highlighter::plain()),
            markdown: MarkdownConfig::default(),
            viewer: ViewerConfig::default(),
            sidebar: SidebarConfig::default(),
            threads: ThreadsConfig::default(),
            diff: DiffConfig::default(),
            agents: AgentsConfig::default(),
            user: UserConfig::default(),
            config_path: PathBuf::from("config.kdl"),
        }
    }
}

#[cfg(test)]
mod tests;
