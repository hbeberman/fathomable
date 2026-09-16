// @okf-doc: /decisions/0012-workspace-mode.md
//! The workspace app: open documents, sidebar, popups, and the state every
//! concept module hangs off.
//!
//! [`App`] is plain state so the viewer's behaviour is tested without a
//! terminal. The modules are grouped by concept (ADR 0048): `threads`
//! holds the thread cursor, the panes, the list, and the store operations;
//! `draw` renders; `input` binds and dispatches keys and the mouse; `agents`
//! holds the human-invoked wake stub; `sidebar` is the column and
//! `files_pane` its upper pane; `view`, `watch`, `socket`, `commands`, and
//! `clipboard` are what their names say; and
//! [`run`] owns the terminal, the file watcher, and the viewer socket.

pub(crate) mod agents;
mod checkpoints;
mod clipboard;
mod commands;
mod diff;
mod diff_keys;
mod doctor_view;
mod draw;
mod file_index;
mod files_pane;
mod files_shown;
mod goto_file;
pub(crate) mod input;
mod jumplist;
mod last_seen;
mod menu_bar;
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
mod worktrees;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::threads::list::ReviewList;
use fathomable_core::annotations::{
    self, ActivityCursor, MessageTarget, ResolutionOutcome, Store, ThreadId,
};
use fathomable_core::config::{
    DiffConfig, JumpConfig, MarkdownConfig, SidebarConfig, ThreadsConfig, UserConfig, ViewerConfig,
    WatchConfig,
};
use fathomable_core::content::Policy;
use fathomable_core::diff::Diff;
use fathomable_core::follow::{Change, Ignore, Queue, Target};
use fathomable_core::highlight::{Highlighter, language_hint};
use fathomable_core::picker::{Match, Picker};
use fathomable_core::reach::Reach;
use fathomable_core::seen;
use fathomable_core::session::{Record, Request, Response};
use fathomable_core::status::{State, Status};
use fathomable_core::tree::Tree;
use fathomable_core::workspace::{EntryKind, Filter, Workspace, WorkspaceError, is_rules_file};
use fathomable_core::{Document, XdgDirs};
use input::bindings::Chord;

pub(crate) use threads::cursor::ThreadCursor;
pub(crate) use threads::{Compose, Mark};
use view::{HunkStep, Syntax, View};
use watch::{Fingerprint, is_git_metadata};

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

/// How a status-line notice should draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeTone {
    Info,
    Warning,
}

/// The answer to the reader's last action.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Notice {
    text: String,
    tone: NoticeTone,
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
    /// The base side of the diff (ADR 0060).
    DiffBase,
    /// The target side of the diff (ADR 0060).
    DiffTarget,
    /// The worktrees of the workspace, the active one marked (ADR 0070).
    Worktree,
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
    Help(input::help::Help),
    /// A file, recent-document, or thread picker.
    Picker(PickerState),
    /// The draft being written in the text (ADR 0013, 0054): a popup
    /// only in that it takes the keys.
    Compose(Compose),
    /// The `:status` overlay (ADR 0021).
    Status,
    /// Shared CLI diagnostics in a scrollable in-app view.
    Doctor(doctor_view::Doctor),
    /// Project identity and repository link.
    About,
    /// The context menu a right-click opened (ADR 0050).
    Menu(input::menu::Menu),
}

#[derive(Debug)]
struct Doc {
    document: Document,
    relative: PathBuf,
    view: View,
    marks: Vec<Mark>,
    /// The draft waiting here while another file or popup has the keys.
    draft: Option<Compose>,
    /// Whether the text has changed or been read since it was last
    /// snapshotted as seen.
    seen_dirty: bool,
    /// Set while the file is gone from disk (ADR 0028); retained content
    /// stays available until it returns.
    deleted: Option<Deleted>,
}

/// Which retained source a deleted file shows (ADR 0028).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deleted {
    /// The file vanished while open, so its last loaded content remains.
    Loaded,
    /// The worktree file is absent; the index blob is shown.
    Index,
    /// The deletion is staged; the `HEAD` blob is shown.
    Head,
}

impl Deleted {
    const fn banner(self) -> &'static str {
        match self {
            Self::Loaded => "deleted from worktree · showing last loaded",
            Self::Index => "deleted from worktree · showing INDEX",
            Self::Head => "staged deletion · showing HEAD",
        }
    }
}

fn deleted_source(entry: Option<&fathomable_core::status::Entry>) -> Option<Deleted> {
    let entry = entry?;
    if entry.unstaged_state() == Some(State::Deleted) {
        Some(Deleted::Index)
    } else if entry.staged_state() == Some(State::Deleted) && entry.unstaged_state().is_none() {
        Some(Deleted::Head)
    } else {
        None
    }
}

fn activity_observation(store: Option<&Store>) -> (Option<PathBuf>, ActivityCursor) {
    store.map_or_else(
        || (None, ActivityCursor::default()),
        |store| (Some(store.path().to_path_buf()), store.activity_cursor()),
    )
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
    /// The reusable first-workspace page is open over the current content;
    /// the saved focus is restored when it closes.
    getting_started: Option<Focus>,
    tree: Option<Tree>,
    /// Directory card shown while the files-pane cursor names a directory.
    directory: Option<files_pane::DirectorySelection>,
    /// The sidebar's panes, scope, split, and sizes (ADR 0049, ADR 0057).
    sidebar: sidebar::Sidebar,
    /// Whether stubs are drawn, and for resolved threads (ADR 0049).
    stubs: threads::stubs::StubState,
    /// The threads expanded in place this session (ADR 0049).
    expanded: HashSet<ThreadId>,
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
    /// The border a mouse drag is moving.
    drag: Option<Border>,
    /// The cell the pointer was last seen at, for hover (ADR 0050).
    pointer: Option<(usize, usize)>,
    /// The last left press in the text, for click counts (ADR 0050).
    press: Option<input::mouse::Press>,
    focus: Focus,
    popup: Option<Popup>,
    menu_bar: menu_bar::MenuBar,
    /// The `Space f` files (ADR 0028), patched as paths come and go.
    file_index: file_index::FileIndex,
    /// The same with ignored files, for `I`.
    all_index: file_index::FileIndex,
    message: Option<Notice>,
    /// The keys typed so far of a longer binding (ADR 0045).
    prefix: Vec<Chord>,
    /// The thread a first `d` armed for deletion (ADR 0034).
    pending_delete: Option<annotations::ThreadId>,
    width: usize,
    height: usize,
    viewer_id: String,
    store: Option<Store>,
    /// Store identity and append-log position already reported as activity.
    activity_store: Option<PathBuf>,
    activity_cursor: ActivityCursor,
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
    /// How the person at the viewer is named (ADR 0058).
    user: UserConfig,
    /// The config file the over-limit notice names (ADR 0026).
    config_path: PathBuf,
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
    /// Whether the recursive workspace watch is in place.
    watching_root: bool,
    /// Whether the dirty set missed a refresh, so the next one must
    /// walk the whole tree rather than build on it.
    status_stale: bool,
    /// The full walks of the dirty set, off the loop (ADR 0017).
    walks: status_walk::Walks,
    /// Every uncommitted path (ADR 0017), refreshed on git and file events.
    status: Status,
    /// The workspace's worktrees as last listed (ADR 0070).
    worktrees: Vec<fathomable_core::worktrees::Worktree>,
    /// The git paths watched for the other worktrees (ADR 0070).
    worktree_paths: Vec<PathBuf>,
    /// What the other worktrees' `HEAD`s reached last time (ADR 0070).
    reach_cache: HashMap<PathBuf, worktrees::ReachEntry>,
    /// Where a thread another worktree shows sits in that worktree's
    /// file (ADR 0070).
    elsewhere: HashMap<ThreadId, annotations::Placement>,
    /// What the loop's watcher must move to, once (ADR 0070).
    rewatch: Option<worktrees::Rewatch>,
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
            menu_bar,
            threads,
            diff,
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
        let (activity_store, activity_cursor) = activity_observation(store.as_ref());
        let mut app = Self {
            workspace,
            docs: Vec::new(),
            current: None,
            recent: Vec::new(),
            jumplist: jumplist::Jumplist::default(),
            search_origin: None,
            welcome: View::new(String::new(), 1, 1),
            getting_started: None,
            tree: None,
            directory: None,
            sidebar: sidebar::Sidebar::new(sidebar),
            stubs: threads::stubs::StubState::from_config(&threads),
            expanded: HashSet::new(),
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
            menu_bar: menu_bar::MenuBar::new(menu_bar),
            file_index: file_index::FileIndex::new(Filter::Visible),
            all_index: file_index::FileIndex::new(Filter::All),
            message: None,
            prefix: Vec::new(),
            pending_delete: None,
            width,
            height,
            viewer_id: record.id().to_string(),
            store,
            activity_store,
            activity_cursor,
            reach: Reach::everything(),
            record,
            dirs,
            jump,
            highlighter,
            markdown,
            viewer,
            user,
            config_path,
            ignore,
            queue: Queue::default(),
            toasts: Vec::new(),
            seen,
            checkpoints,
            diff_choices: Vec::new(),
            diff_target_next: None,
            compare: diff.compare(),
            watching_root: true,
            status_stale: false,
            walks: status_walk::Walks::new(),
            status: Status::default(),
            worktrees: Vec::new(),
            worktree_paths: Vec::new(),
            reach_cache: HashMap::new(),
            elsewhere: HashMap::new(),
            rewatch: None,
        };
        if app.sidebar.tree && !app.ensure_tree() {
            app.sidebar.tree = false;
        }
        app.relayout();
        app.refresh_worktrees();
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
        let head = self.workspace.head_commit();
        let active = match (self.store.as_mut(), head) {
            (Some(store), Some(head)) => match self.workspace.reachable(store.commits()) {
                Some(mut reachable) => {
                    if crate::app::threads::reach::follow_head(store, &self.workspace, &reachable)
                        > 0
                    {
                        reachable.insert(head.clone());
                    }
                    Some((head, reachable))
                }
                None => None,
            },
            _ => None,
        };
        self.reconcile_agent_activity();
        // The other worktrees widen the reach (ADR 0070).
        let scope = match active {
            Some((head, reachable)) => self.reach_with_others(head, reachable),
            None => Reach::everything(),
        };
        if scope != self.reach {
            tracing::info!(head = ?self.workspace.head_commit(), "thread reach changed");
            self.reach = scope;
            for index in 0..self.docs.len() {
                self.refresh_marks(index);
            }
        }
        self.refresh_elsewhere();
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
                if changed {
                    tracing::info!(threads = store.threads().len(), "thread store reloaded");
                }
                self.store = Some(store);
                self.reconcile_agent_activity();
                if !changed {
                    return;
                }
                self.refresh_reach();
                for index in 0..self.docs.len() {
                    self.refresh_marks(index);
                }
            }
            Err(error) => tracing::warn!(%error, "cannot reload the thread store"),
        }
    }

    /// Report newly observed agent messages from the append-only store.
    pub(super) fn reconcile_agent_activity(&mut self) {
        let Some(store) = self.store.as_ref() else {
            self.activity_store = None;
            self.activity_cursor = ActivityCursor::default();
            return;
        };
        let path = store.path();
        let end = store.activity_cursor();
        if self.activity_store.as_deref() != Some(path) {
            tracing::info!(
                path = %path.display(),
                cursor = end.ordinal(),
                "agent activity observation seeded for replacement store"
            );
            self.activity_store = Some(path.to_path_buf());
            self.activity_cursor = end;
            return;
        }
        if end < self.activity_cursor {
            tracing::warn!(
                path = %path.display(),
                observed = self.activity_cursor.ordinal(),
                current = end.ordinal(),
                "thread store activity cursor regressed; reseeding without replay"
            );
            self.activity_cursor = end;
            return;
        }

        let activities = store
            .agent_activity_since(self.activity_cursor)
            .collect::<Vec<_>>();
        if activities.is_empty() {
            self.activity_cursor = end;
            return;
        }
        let notification = if activities.len() == 1 {
            let activity = activities[0];
            let agent = threads::author_label(activity.author(), self.user_name());
            let place = store.thread(activity.thread()).map_or_else(
                || threads::file::toast_place_at(activity.path(), activity.range()),
                threads::file::toast_place,
            );
            match (activity.target(), activity.resolution()) {
                (MessageTarget::Comment, _) => {
                    format!("{agent} started a thread on {place}")
                }
                (MessageTarget::Reply(_), ResolutionOutcome::NotRequested) => {
                    format!("{agent} replied on {place}")
                }
                (MessageTarget::Reply(_), ResolutionOutcome::ResolutionProposed) => {
                    format!("{agent} replied and proposed resolution on {place}")
                }
                (MessageTarget::Reply(_), ResolutionOutcome::Resolved) => {
                    format!("{agent} replied and resolved {place}")
                }
            }
        } else {
            format!("{} agent updates", activities.len())
        };
        self.push_toast(notification);
        self.activity_cursor = end;
    }

    /// Where the thread store lives, for the watcher.
    pub(crate) fn store_path(&self) -> Option<&Path> {
        self.store.as_ref().map(Store::path)
    }

    /// `:name`: label this viewer window; empty clears the name.
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

    /// Whether every visible workspace directory is watched.
    pub(crate) fn set_watching_root(&mut self, watching: bool) -> bool {
        if self.watching_root == watching {
            return false;
        }
        self.watching_root = watching;
        if !watching {
            self.notice(
                "cannot watch every visible directory; live updates have partial coverage (--doctor counts the directories)",
            );
        }
        true
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
        self.reload_state_named_in(&events);
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
            let Some(event) = on_this_side(event, &root) else {
                continue;
            };
            let Some(path) = event.path() else {
                continue;
            };
            // A `HEAD`, a ref, or the registry of another worktree moved
            // (ADR 0070); outside the root, so it is not a file event.
            if self.is_worktree_path(path) {
                git_changed = true;
                continue;
            }
            let Ok(relative) = path.strip_prefix(&root).map(Path::to_path_buf) else {
                continue;
            };
            if relative.starts_with(".git") {
                git_changed |= is_git_metadata(&relative)
                    || relative.starts_with(".git/worktrees")
                    || relative.starts_with(".git/refs");
                continue;
            }
            if let watch::Event::Renamed { from, .. } = &event
                && let Ok(from) = from.strip_prefix(&root)
            {
                changed.push(from.to_path_buf());
            }
            changed.push(relative.clone());
            let created = matches!(&event, watch::Event::Created(_));
            match event {
                watch::Event::Change(absolute) | watch::Event::Created(absolute) => {
                    if !seen_paths.insert(relative.clone()) {
                        continue;
                    }
                    let listed = self
                        .tree
                        .as_ref()
                        .is_some_and(|tree| tree.contains(&relative));
                    if !listed
                        || created
                        || self
                            .status
                            .get(&relative)
                            .is_some_and(|entry| entry.state() == State::Deleted)
                    {
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
            self.on_git_changed();
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
            self.refresh_directory_selection();
        }
    }

    /// Reload append-only state files that another process changed.
    fn reload_state_named_in(&mut self, events: &[watch::Event]) {
        // The thread store lives outside the root and arrives through the
        // state directory watch (ADR 0024).
        if events.iter().any(|event| event.path() == self.store_path()) {
            self.reload_store();
        }
    }

    /// `HEAD`, the index, a ref, or the worktree set moved: the worktrees
    /// are listed again (ADR 0070), the `HEAD` bases re-read, and the
    /// reach recomputed.
    fn on_git_changed(&mut self) {
        tracing::info!("git metadata changed; refreshing HEAD bases");
        self.refresh_worktrees();
        for index in 0..self.docs.len() {
            self.refresh_base(index);
        }
        self.refresh_reach();
    }

    /// Reconcile state after the platform reports that watcher events were
    /// lost. The tree, loaded documents, repository bases, status, and
    /// annotation store may all have changed during the gap.
    fn rescan_workspace(&mut self) {
        tracing::warn!("file watcher lost events; rescanning workspace");
        self.reload_rules();
        self.reload_store();
        self.refresh_worktrees();
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
        match self.workspace.reload_rules() {
            Ok(()) => {
                let rewatch = self.rewatch.get_or_insert(worktrees::Rewatch {
                    root: None,
                    extras: Vec::new(),
                });
                rewatch.root = Some(self.workspace.root().to_path_buf());
            }
            Err(error) => self.notice(format!("ignore rules: {error}")),
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
            doc.deleted = Some(Deleted::Loaded);
            doc.view.set_worktree_missing(true);
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
            doc.view.set_worktree_missing(false);
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
                let shown = self
                    .current
                    .and_then(|index| self.docs.get(index))
                    .and_then(|doc| doc.view.diff())
                    .map(|diff| (diff.base.clone(), diff.target.clone()));
                self.status = status;
                self.status_stale = false;
                let root = self.workspace.root().to_path_buf();
                let deleted_updates: Vec<(usize, Deleted, PathBuf)> = self
                    .docs
                    .iter()
                    .enumerate()
                    .filter_map(|(index, doc)| {
                        let missing = !root.join(&doc.relative).is_file();
                        let source = missing
                            .then(|| deleted_source(self.status.get(&doc.relative)))
                            .flatten()?;
                        Some((index, source, doc.relative.clone()))
                    })
                    .collect();
                let mut marks_to_refresh = Vec::new();
                for (index, source, relative) in deleted_updates {
                    let current = self.docs[index].deleted;
                    if current.is_none() {
                        self.docs[index].deleted = Some(Deleted::Loaded);
                        self.docs[index].view.set_worktree_missing(true);
                    } else if current != Some(Deleted::Loaded) {
                        let bytes = match source {
                            Deleted::Index => self.workspace.index_bytes(&relative),
                            Deleted::Head => self.workspace.head_bytes(&relative),
                            Deleted::Loaded => unreachable!("loaded content is not a Git source"),
                        };
                        match bytes {
                            Ok(Some(bytes)) => {
                                let changed = self.docs[index]
                                    .document
                                    .replace_snapshot(bytes)
                                    .map_err(|error| error.to_string());
                                match changed {
                                    Ok(changed) => {
                                        self.docs[index].deleted = Some(source);
                                        if changed {
                                            let text = self.docs[index]
                                                .document
                                                .text()
                                                .unwrap_or_default()
                                                .to_owned();
                                            self.docs[index].view.reload(text);
                                            marks_to_refresh.push(index);
                                        }
                                        self.docs[index].view.set_worktree_missing(true);
                                    }
                                    Err(error) => {
                                        tracing::warn!(%error, "cannot refresh deleted snapshot");
                                        self.notice(error);
                                    }
                                }
                            }
                            Ok(None) => self.notice(format!(
                                "no Git snapshot is available for {}",
                                relative.display()
                            )),
                            Err(error) => self.notice(error.to_string()),
                        }
                    }
                }
                for index in marks_to_refresh {
                    self.refresh_marks(index);
                }
                for doc in &mut self.docs {
                    doc.view.set_index_missing(
                        self.status
                            .get(&doc.relative)
                            .is_some_and(|entry| entry.staged_state() == Some(State::Deleted)),
                    );
                }
                self.sift_tree();
                if let Some((base, target)) = shown {
                    self.show_diff(base, target);
                }
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
            self.docs[index].view.set_worktree_missing(false);
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

    /// Expire toasts and snapshot the current file once it has been idle.
    pub(crate) fn tick(&mut self) {
        let now = Instant::now();
        self.toasts.retain(|toast| toast.until > now);
        if let Some(index) = self.current
            && self.docs[index].seen_dirty
            && self.docs[index].view.idle() >= self.viewer.seen_idle
        {
            self.mark_seen(index);
        }
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
        next
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
        self.message
            .as_ref()
            .map(|notice| notice.text.as_str())
            .or_else(|| self.view().message())
    }

    /// The visual urgency of the current status-line notice.
    pub(crate) fn message_tone(&self) -> NoticeTone {
        self.message
            .as_ref()
            .map_or(NoticeTone::Info, |notice| notice.tone)
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

    /// Whether the current document's file is gone from disk (ADR 0028).
    pub(crate) fn deleted(&self) -> bool {
        self.current
            .and_then(|i| self.docs.get(i))
            .is_some_and(|doc| doc.deleted.is_some())
    }

    /// The banner over retained source while the worktree file is gone.
    pub(crate) fn banner(&self) -> Option<&'static str> {
        if self.getting_started() || self.directory_path().is_some() {
            return None;
        }
        self.current
            .and_then(|i| self.docs.get(i))
            .filter(|_| !self.review_list.is_open())
            .and_then(|doc| doc.deleted.map(Deleted::banner))
    }

    /// Rows available to panes once the status line is taken.
    pub(crate) fn pane_rows(&self) -> usize {
        self.height
            .saturating_sub(1 + usize::from(self.menu_bar.shown()))
            .max(1)
    }

    /// First screen row occupied by the panes.
    pub(crate) fn pane_top(&self) -> usize {
        usize::from(self.menu_bar.shown())
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
    /// 0067): the column shows a document, and a diff is open (ADR
    /// 0069), a draft is open, a thread is under the cursor, or the file
    /// has a thread to fold. The review list and the file-info pane have
    /// no bar of this kind.
    pub(crate) fn text_bar_shown(&self) -> bool {
        self.has_document()
            && !self.getting_started()
            && self.directory_path().is_none()
            && !self.review_list().is_open()
            && self.info().is_none()
            && (self.view().diff_view()
                || self.draft().is_some()
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
        self.pane_top()
            + usize::from(self.banner().is_some())
            + usize::from(self.diff_chrome_rows() > 0)
    }

    pub(crate) fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.relayout();
        // Message and draft rows wrap at the new width (ADR 0049, 0054).
        self.place_stub_rows();
        input::help::resize(self);
        doctor_view::resize(self);
        menu_bar::resize(self);
    }

    fn relayout(&mut self) {
        let rows = self
            .text_rows()
            .saturating_sub(usize::from(self.text_bar_shown()));
        let sidebar = self.sidebar_width();
        let text_width = self
            .width
            .saturating_sub(sidebar)
            .saturating_sub(crate::app::draw::gutter_width(self.view()))
            .max(1);
        self.view_mut().resize(text_width, rows);
        self.scroll_tree();
    }

    /// Scrolling uses only rows the key bar does not cover.
    fn sync_text_height(&mut self) {
        let rows = self
            .text_rows()
            .saturating_sub(usize::from(self.text_bar_shown()));
        self.view_mut().set_height(rows);
    }

    pub(crate) fn clear_message(&mut self) {
        self.message = None;
    }

    /// Raise a toast for `jump.toast`, dropping the oldest past the cap.
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
        self.message = Some(Notice {
            text: message,
            tone: NoticeTone::Info,
        });
    }

    fn warning(&mut self, message: impl Into<String>) {
        let message = message.into();
        tracing::warn!(message = %message, "status warning");
        self.message = Some(Notice {
            text: message,
            tone: NoticeTone::Warning,
        });
    }

    /// Open the root-relative `path`, loading it or switching to it.
    pub(crate) fn open(&mut self, path: &Path) {
        self.getting_started = None;
        let had_directory = self.directory.take().is_some();
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
            let deleted = matches!(absolute.try_exists(), Ok(false))
                .then(|| deleted_source(self.status.get(&relative)))
                .flatten();
            let document = match deleted {
                Some(Deleted::Index) => self
                    .workspace
                    .index_bytes(&relative)
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| {
                        bytes
                            .ok_or_else(|| "no index snapshot is available".to_owned())
                            .and_then(|bytes| {
                                Document::from_snapshot(&absolute, bytes, policy)
                                    .map_err(|error| error.to_string())
                            })
                    }),
                Some(Deleted::Head) => self
                    .workspace
                    .head_bytes(&relative)
                    .map_err(|error| error.to_string())
                    .and_then(|bytes| {
                        bytes
                            .ok_or_else(|| "no HEAD snapshot is available".to_owned())
                            .and_then(|bytes| {
                                Document::from_snapshot(&absolute, bytes, policy)
                                    .map_err(|error| error.to_string())
                            })
                    }),
                Some(Deleted::Loaded) => unreachable!("new documents have no loaded snapshot"),
                None => Document::load(&absolute, policy).map_err(|error| error.to_string()),
            };
            match document {
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
                    view.set_worktree_missing(deleted.is_some());
                    view.set_index_missing(
                        self.status
                            .get(&relative)
                            .is_some_and(|entry| entry.staged_state() == Some(State::Deleted)),
                    );
                    self.docs.push(Doc {
                        document,
                        relative: relative.clone(),
                        view,
                        marks: Vec::new(),
                        draft: None,
                        seen_dirty: true,
                        deleted,
                    });
                    self.docs.len() - 1
                }
                Err(error) => {
                    self.notice(format!("{error:#}"));
                    if had_directory {
                        self.relayout();
                    }
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
            Request::ThreadReply {
                thread,
                author,
                body,
                resolve,
                lines,
                idempotency_key,
                caller,
            } => match self.agent_reply(
                &thread,
                author,
                body,
                caller,
                resolve,
                lines,
                idempotency_key,
            ) {
                Ok((thread, resolution, replayed)) => {
                    if replayed {
                        Response::thread_reply_replayed(thread, resolution)
                    } else {
                        Response::thread_reply_applied(thread, resolution)
                    }
                }
                Err(error) => Response::Error(error),
            },
            Request::ThreadStart {
                path,
                range,
                author,
                body,
                idempotency_key,
                caller,
            } => match self.agent_start(&path, range, author, body, caller, idempotency_key) {
                Ok((thread, _)) => Response::Threads(vec![thread]),
                Err(error) => Response::Error(error),
            },
        }
    }

    fn show(&mut self, index: usize) {
        self.directory = None;
        if let Some(previous) = self.current
            && previous != index
        {
            self.park_draft();
            self.mark_seen(previous);
        }
        self.current = Some(index);
        // A document takes the column back from the list (ADR 0025).
        self.review_list.close();
        self.focus = Focus::View;
        self.refresh_base(index);
        self.refresh_marks(index);
        // The document's stubs follow the session's toggles (ADR 0049).
        self.place_stub_rows();
        self.relayout();
        self.resume_draft();
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

    /// A far move outside key dispatch records where it started.
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
        let shown = self
            .docs
            .get(index)
            .and_then(|doc| doc.view.diff())
            .map(|diff| (diff.base.clone(), diff.target.clone()));
        if let Some(doc) = self.docs.get_mut(index) {
            doc.view.set_bases(seen, staged, head);
            doc.view.set_index_missing(
                self.status
                    .get(&relative)
                    .is_some_and(|entry| entry.staged_state() == Some(State::Deleted)),
            );
        }
        if self.current == Some(index)
            && let Some((base, target)) = shown
        {
            self.show_diff(base, target);
        }
    }

    // ----- sidebar -----

    fn ensure_tree(&mut self) -> bool {
        if self.tree.is_some() {
            return true;
        }
        match Tree::new(&mut self.workspace) {
            Ok(mut tree) => {
                tree.sift(&self.status);
                self.tree = Some(tree);
                true
            }
            Err(error) => {
                self.notice(error.to_string());
                false
            }
        }
    }

    /// What a start shows: the configured layout around the named file or
    /// welcome page, with an explicitly named file retaining text focus.
    pub(crate) fn start_on(&mut self, open: Option<&Path>) {
        if let Some(path) = open {
            self.open(path);
        } else if self.sidebar.tree {
            self.focus = Focus::Tree;
        } else if self.sidebar.threads {
            self.focus = Focus::ThreadsPane;
        }
    }

    /// Show and focus the files pane.
    #[cfg(test)]
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
            self.sidebar.show_tree();
            self.reveal_current();
            self.focus = Focus::Tree;
            self.show_highlight();
        } else if self.focus == Focus::Tree {
            self.focus = Focus::View;
        } else {
            self.reveal_current();
            self.focus = Focus::Tree;
            self.show_highlight();
        }
        self.relayout();
    }

    /// `Space p f`: hide the files pane, or show it again without taking
    /// the keys; the threads pane keeps the sidebar either way.
    pub(crate) fn toggle_tree_shown(&mut self) {
        if self.sidebar.tree {
            self.sidebar.hide_tree();
            if self.focus == Focus::Tree {
                self.focus = Focus::View;
            }
        } else {
            if !self.ensure_tree() {
                return;
            }
            self.sidebar.show_tree();
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
        self.park_draft();
        self.popup = Some(Popup::Help(input::help::Help::default()));
    }

    /// `:status`: the overlay of session facts (ADR 0021).
    pub(crate) fn open_status(&mut self) {
        self.park_draft();
        self.popup = Some(Popup::Status);
    }

    pub(crate) fn close_popup(&mut self) {
        self.park_draft();
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
            PickerKind::DiffBase | PickerKind::DiffTarget => self.diff_choices(),
            PickerKind::Worktree => self.worktree_choices(),
        };
        tracing::info!(?kind, items = items.len(), "picker opened");
        self.park_draft();
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
            Some((kind @ (PickerKind::DiffBase | PickerKind::DiffTarget), item)) => {
                self.choose_diff_side(kind, &item);
            }
            Some((PickerKind::Worktree, item)) => self.choose_worktree(&item),
            None => {}
        }
    }
}

/// `event` as seen from inside `root`: a move across the root's edge is
/// a plain arrival or departure on this side of it, and a move wholly
/// outside is nothing.
fn on_this_side(event: watch::Event, root: &Path) -> Option<watch::Event> {
    match event {
        watch::Event::Renamed { from, to } => {
            match (from.starts_with(root), to.starts_with(root)) {
                (true, true) => Some(watch::Event::Renamed { from, to }),
                (true, false) => Some(watch::Event::Removed(from)),
                (false, true) => Some(watch::Event::Created(to)),
                (false, false) => None,
            }
        }
        other => Some(other),
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
    /// Change notification settings (ADR 0015).
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
    /// Whether the persistent menu bar starts visible.
    pub(crate) menu_bar: bool,
    /// How threads show in the text (ADR 0049).
    pub(crate) threads: ThreadsConfig,
    /// How diffs are compared and listed (ADR 0060).
    pub(crate) diff: DiffConfig,
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
            record: Record::new(Id::mint(), root.clone(), root, None),
            // State a test app writes (a record, a marker) lands in the
            // temp dir, never beside the sources.
            dirs: XdgDirs::resolve(|name| {
                (name == "XDG_STATE_HOME")
                    .then(|| std::env::temp_dir().join("fathomable-test-state").into())
            }),
            store: None,
            jump: JumpConfig::default(),
            watch: WatchConfig::default(),
            seen: None,
            checkpoints: None,
            highlighter: Arc::new(Highlighter::plain()),
            markdown: MarkdownConfig::default(),
            viewer: ViewerConfig::default(),
            sidebar: SidebarConfig {
                visible: false,
                ..SidebarConfig::default()
            },
            menu_bar: false,
            threads: ThreadsConfig::default(),
            diff: DiffConfig::default(),
            user: UserConfig::default(),
            config_path: PathBuf::from("config.kdl"),
        }
    }
}

#[cfg(test)]
mod tests;
