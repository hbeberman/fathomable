// @okf-doc: /decisions/0012-workspace-mode.md
//! The workspace app: open documents, sidebar, popups, and the state every
//! concept module hangs off.
//!
//! [`App`] is plain state so the viewer's behaviour is tested without a
//! terminal. The modules are grouped by concept (ADR 0048): `threads`
//! holds the thread cursor, the panes, the list, and the store operations;
//! `draw` renders; `input` binds and dispatches keys and the mouse; `agents`
//! holds the human-invoked wake stub; `sidebar` is the column and
//! `files_pane` its upper pane; `view`, `watch`, `commands`, and `clipboard`
//! are what their names say; and [`run`] owns the terminal and file watcher.

pub(crate) mod agents;
mod clipboard;
mod commands;
mod comparison;
mod diff;
mod diff_keys;
mod doctor_view;
mod draw;
mod file_index;
mod files_pane;
mod files_shown;
mod goto_file;
mod highlight;
pub(crate) mod input;
mod jumplist;
mod licenses;
mod menu_bar;
mod navigation;
mod review_points;
pub(crate) mod run;
mod sidebar;
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
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::threads::list::ReviewList;
use fathomable_core::annotations::{
    self, ActivityCursor, MessageTarget, ResolutionOutcome, Store, StoreError, ThreadId,
};
use fathomable_core::config::{
    DiffConfig, DiffMode, MarkdownConfig, SidebarConfig, ThreadsConfig, UserConfig, ViewerConfig,
    WatchConfig,
};
use fathomable_core::content::Policy;
use fathomable_core::diff::Diff;
use fathomable_core::follow::Ignore;
use fathomable_core::highlight::{Highlighter, language_hint};
use fathomable_core::layout::LineIndex;
use fathomable_core::picker::{Match, Picker};
use fathomable_core::reach::Reach;
use fathomable_core::session::Record;
use fathomable_core::status::{State, Status};
use fathomable_core::tree::Tree;
use fathomable_core::workspace::{
    ComparisonEndpoint, EntryKind, Filter, Workspace, WorkspaceError, is_rules_file,
};
use fathomable_core::{Document, XdgDirs};
use input::bindings::Chord;

pub(crate) use threads::cursor::ThreadCursor;
pub(crate) use threads::{Compose, Mark};
use view::{Syntax, View};
use watch::{Fingerprint, is_git_metadata};

/// Toasts visible at once.
pub(crate) const MAX_TOASTS: usize = 3;

const THREAD_WATCH_DEGRADED: &str =
    "cannot watch or reconcile the thread store; keeping the last loaded board";

/// Long enough to read at startup without becoming steady status.
const STARTUP_WARNING_DURATION: Duration = Duration::from_secs(8);

/// A transient one-line notice about a change (ADR 0015).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Toast {
    text: String,
    kind: ToastKind,
    until: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToastKind {
    Plain,
    FileEdit {
        path: PathBuf,
        added: usize,
        removed: usize,
    },
}

impl Toast {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn kind(&self) -> &ToastKind {
        &self.kind
    }
}

/// How a status-line notice should draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoticeTone {
    Info,
    Warning,
    Error,
}

/// The answer to the reader's last action.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Notice {
    text: String,
    tone: NoticeTone,
    until: Option<Instant>,
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
    /// The base endpoint of the viewer-wide comparison.
    ComparisonBase,
    /// The target endpoint of the viewer-wide comparison.
    ComparisonTarget,
    /// Tags offered for one comparison side.
    ComparisonTags(ComparisonSide),
    /// Local and remote-tracking branches offered for one comparison side.
    ComparisonBranches(ComparisonSide),
    /// Commits reachable from one selected branch.
    ComparisonBranchCommits(ComparisonSide),
    /// Saved review points offered as comparison bases.
    ComparisonReviewPoints,
    /// Uncommon endpoint choices for one comparison side.
    ComparisonAdvanced(ComparisonSide),
    /// Optional name for a new workspace review point.
    ReviewPointName,
    /// The worktrees of the workspace, the active one marked (ADR 0070).
    Worktree,
}

/// Which side a nested comparison picker will replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComparisonSide {
    Base,
    Target,
}

impl ComparisonSide {
    const fn picker_kind(self) -> PickerKind {
        match self {
            Self::Base => PickerKind::ComparisonBase,
            Self::Target => PickerKind::ComparisonTarget,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Target => "target",
        }
    }
}

#[derive(Debug, Clone)]
struct PickerSearch {
    prefix: String,
    rows: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitSearch {
    revision: Option<String>,
    prefix: String,
}

/// The open picker popup.
#[derive(Debug)]
pub(crate) struct PickerState {
    kind: PickerKind,
    picker: Picker,
    base_items: Vec<String>,
    input: String,
    matches: Vec<Match>,
    selected: usize,
    scroll: usize,
    scope: Option<String>,
    id_search: Option<PickerSearch>,
}

impl PickerState {
    #[cfg(test)]
    fn new(kind: PickerKind, items: Vec<String>) -> Self {
        Self::scoped(kind, items, None)
    }

    fn scoped(kind: PickerKind, items: Vec<String>, scope: Option<String>) -> Self {
        let mut picker = Picker::new(items);
        let matches = picker.query("");
        Self {
            kind,
            base_items: picker.items().to_vec(),
            picker,
            input: String::new(),
            matches,
            selected: 0,
            scroll: 0,
            scope,
            id_search: None,
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

    pub(crate) fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    pub(crate) fn item(&self, m: &Match) -> &str {
        &self.picker.items()[m.index()]
    }

    /// Number of candidates before filtering.
    pub(crate) fn total(&self) -> usize {
        self.picker.items().len()
    }

    pub(crate) fn matched(&self) -> usize {
        self.matches.len()
    }

    pub(crate) fn first_visible(&self, rows: usize) -> usize {
        if rows == 0 {
            return 0;
        }
        let margin = PICKER_SCROLLOFF.min(rows.saturating_sub(1) / 2);
        let last_first = self.matches.len().saturating_sub(rows);
        let mut first = self.scroll.min(last_first);
        if self.selected < first.saturating_add(margin) {
            first = self.selected.saturating_sub(margin);
        } else if self.selected > first.saturating_add(rows).saturating_sub(margin + 1) {
            first = self
                .selected
                .saturating_add(margin + 1)
                .saturating_sub(rows);
        }
        first.min(last_first)
    }

    fn move_by(&mut self, delta: isize, rows: usize) {
        if self.matches.is_empty() {
            return;
        }
        self.scroll = self.first_visible(rows);
        self.selected = self
            .selected
            .saturating_add_signed(delta)
            .min(self.matches.len() - 1);
        self.scroll = self.first_visible(rows);
    }

    fn commit_search_request(&mut self) -> Option<CommitSearch> {
        let revision = match self.kind {
            PickerKind::ComparisonBase | PickerKind::ComparisonTarget => None,
            PickerKind::ComparisonBranchCommits(_) => self.scope.clone(),
            _ => {
                self.requery();
                return None;
            }
        };
        let prefix = self.input.trim().to_ascii_lowercase();
        if prefix.len() < 4 || !prefix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            self.id_search = None;
            self.replace_items(Vec::new());
            return None;
        }
        if let Some(search) = &self.id_search
            && prefix.starts_with(&search.prefix)
        {
            let rows = search
                .rows
                .iter()
                .filter(|row| row.starts_with(&prefix))
                .cloned()
                .collect();
            self.replace_commit_items(&prefix, rows);
            return None;
        }
        self.requery();
        Some(CommitSearch { revision, prefix })
    }

    fn set_commit_search(&mut self, prefix: &str, rows: Vec<String>) {
        self.id_search = Some(PickerSearch {
            prefix: prefix.to_owned(),
            rows: rows.clone(),
        });
        self.replace_commit_items(prefix, rows);
    }

    fn replace_items(&mut self, extra: Vec<String>) {
        let mut items = self.base_items.clone();
        append_unique_commits(&mut items, extra);
        self.picker = Picker::new(items);
        self.requery();
    }

    fn replace_commit_items(&mut self, prefix: &str, extra: Vec<String>) {
        let mut items: Vec<_> = self
            .base_items
            .iter()
            .filter(|item| {
                comparison::commit_id_from_row(item).is_some_and(|id| id.starts_with(prefix))
            })
            .cloned()
            .collect();
        append_unique_commits(&mut items, extra);
        self.picker = Picker::new(items);
        self.requery();
    }

    fn requery(&mut self) {
        self.matches = self.picker.query(&self.input);
        self.selected = 0;
        self.scroll = 0;
    }
}

/// Rows retained above and below a picker cursor during ordinary scrolling.
const PICKER_SCROLLOFF: usize = 3;

/// List rows inside the picker popup for the current pane height.
pub(crate) fn picker_list_rows(pane_rows: usize) -> usize {
    // The popup leaves two pane rows outside, is 3-20 rows tall, and has
    // one border row above and below its list.
    pane_rows.saturating_sub(2).clamp(3, 20).saturating_sub(2)
}

fn append_unique_commits(items: &mut Vec<String>, extra: Vec<String>) {
    for item in extra {
        let id = comparison::commit_id_from_row(&item);
        let duplicate = id.is_some_and(|id| {
            items
                .iter()
                .any(|candidate| comparison::commit_id_from_row(candidate) == Some(id))
        });
        if !duplicate {
            items.push(item);
        }
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
    /// Bundled first- and third-party license notices.
    Licenses(licenses::Licenses),
    /// Project identity and repository link.
    About,
    /// The context menu a right-click opened (ADR 0050).
    Menu(input::menu::Menu),
    /// The compact diff-mode chooser anchored to a pane header.
    DiffMode(input::menu::ModeMenu),
    /// Guarded confirmation opened by bare `q`.
    ConfirmQuit,
    /// Confirmation for a repository-wide clear-board operation.
    ConfirmBoard {
        slate: annotations::BoardSlate,
        counts: threads::archive::BoardCounts,
        changed: bool,
    },
}

#[derive(Debug)]
struct Doc {
    id: u64,
    document: Document,
    relative: PathBuf,
    view: View,
    marks: Vec<Mark>,
    /// The draft waiting here while another file or popup has the keys.
    draft: Option<Compose>,
    /// Set while the file is gone from disk (ADR 0028); retained content
    /// stays available until it returns.
    deleted: Option<Deleted>,
    /// Why the selected comparison cannot present text for this path.
    comparison_notice: Option<String>,
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
    /// The selected comparison target has no path; its base is shown.
    ComparisonBase,
}

impl Deleted {
    const fn banner(self) -> &'static str {
        match self {
            Self::Loaded => "deleted from worktree · showing last loaded",
            Self::Index => "deleted from worktree · showing INDEX",
            Self::Head => "staged deletion · showing HEAD",
            Self::ComparisonBase => "deleted in comparison · showing base",
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

/// Result of reconciling the in-memory board with its backing file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StoreReload {
    pub(crate) changed: bool,
    pub(crate) healthy: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoreBacking {
    InitiallyAbsent,
    Observed,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThreadWatchCoverage {
    Covered,
    Degraded,
}

fn store_backing(
    store: Option<&Store>,
    dirs: &XdgDirs,
    record: &Record,
) -> (PathBuf, StoreBacking) {
    let path = store.map_or_else(
        || dirs.threads_file(record.key()),
        |store| store.path().to_path_buf(),
    );
    let observed = store.map_or_else(
        || fs::symlink_metadata(&path).is_ok(),
        Store::backing_file_observed,
    );
    let backing = if observed {
        StoreBacking::Observed
    } else {
        StoreBacking::InitiallyAbsent
    };
    (path, backing)
}

fn stable_store_reload(before: Option<&fs::Metadata>, store: &Store, path: &Path) -> bool {
    match (
        before,
        store.backing_file_observed(),
        fs::symlink_metadata(path),
    ) {
        (Some(before), true, Ok(after)) => {
            before.dev() == after.dev() && before.ino() == after.ino()
        }
        (None, false, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => true,
        _ => false,
    }
}

fn watch_ignore(watch: &WatchConfig) -> Ignore {
    Ignore::new(&watch.ignore).unwrap_or_else(|error| {
        tracing::warn!(%error, "ignoring watch.ignore");
        Ignore::default()
    })
}

/// All application state.
#[derive(Debug)]
pub(crate) struct App {
    workspace: Workspace,
    docs: Vec<Doc>,
    highlights: highlight::Queue,
    current: Option<usize>,
    /// Documents by index, most recently shown first (`Space F r`).
    recent: Vec<usize>,
    jumplist: jumplist::Jumplist,
    /// The exact comparison stop reached by the last `J` or `K`.
    change_stop: Option<navigation::ChangeStop>,
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
    /// Current document's already placed inline stubs.
    inline_stubs: Vec<threads::stubs::Stub>,
    /// Expanded message layouts retained across draws and file switches.
    expanded_layout_cache: Vec<(ThreadId, draw::message::ExpandedLayout)>,
    /// Rendered thread bodies shared by the File and Reviews surfaces.
    message_layout_cache: draw::message::MessageLayoutCache,
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
    /// Stable path retained even while opening the store fails.
    thread_store_path: PathBuf,
    /// Whether a real backing file has ever been loaded successfully.
    store_backing: StoreBacking,
    /// Why the thread store could not open, retained for user-facing diagnostics.
    thread_store_error: Option<StoreError>,
    /// Current store reload failure, independently of watcher coverage.
    thread_store_degraded: Option<String>,
    /// Whether the exact store and its state-directory lifecycle are watched.
    thread_watch_coverage: ThreadWatchCoverage,
    /// Current live-update failure, suppressed until its wording changes.
    thread_updates_degraded: Option<String>,
    /// Store identity and append-log position already reported as activity.
    activity_store: Option<PathBuf>,
    activity_cursor: ActivityCursor,
    /// Which threads the current `HEAD` shows (ADR 0024).
    reach: Reach,
    record: Record,
    dirs: XdgDirs,
    /// Shared lifetime of file-edit and activity toasts.
    toast_duration: Duration,
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
    toasts: Vec<Toast>,
    /// Explicit workspace review points.
    review_points: Option<fathomable_core::review_points::ReviewPointStore>,
    /// The one comparison selection for this checkout.
    comparison: comparison::State,
    /// Cached path/count projection for the current comparison generation.
    comparison_status: Status,
    /// The explicitly selected session-wide diff presentation.
    diff_mode: DiffMode,
    /// The active presentation restored when Base is selected while Off.
    last_active_diff_mode: DiffMode,
    /// Target-only path boundary while diff presentation is Off.
    off_target_paths: Option<Vec<PathBuf>>,
    /// The changed-only Files rule retained while Off does not apply it.
    dormant_changed_filter: bool,
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
    /// Exact file renames observed in this active checkout.
    ///
    /// This viewer-local projection prevents one worktree from rewriting the
    /// repository board's path for every other worktree.
    local_thread_paths: HashMap<ThreadId, PathBuf>,
    /// What the loop's watcher must move to, once (ADR 0070).
    rewatch: Option<worktrees::Rewatch>,
}

impl App {
    /// Start with no document open, for a terminal of `width` by `height`.
    ///
    /// Start the viewer with repository comparison and thread state loaded.
    #[expect(
        clippy::too_many_lines,
        reason = "App construction lists each independent viewer state field explicitly."
    )]
    pub(crate) fn new(workspace: Workspace, width: usize, height: usize, options: Options) -> Self {
        let Options {
            record,
            dirs,
            store,
            thread_store_error,
            watch,
            review_points,
            highlighter,
            markdown,
            viewer,
            sidebar,
            menu_bar,
            threads,
            diff,
            user,
            config_path,
            shared_state_ancestor_count,
        } = options;
        let toast_duration = watch.toast;
        let ignore = watch_ignore(&watch);
        let (thread_store_path, store_backing) = store_backing(store.as_ref(), &dirs, &record);
        let (activity_store, activity_cursor) = activity_observation(store.as_ref());
        let comparison = comparison::State::load(&dirs, &workspace, diff.compare());
        let diff_mode = diff.mode;
        let last_active_diff_mode = match diff_mode {
            DiffMode::Unified => DiffMode::Unified,
            DiffMode::Standard | DiffMode::Off => DiffMode::Standard,
        };
        let mut app = Self {
            workspace,
            docs: Vec::new(),
            highlights: highlight::Queue::new(),
            current: None,
            recent: Vec::new(),
            jumplist: jumplist::Jumplist::default(),
            change_stop: None,
            search_origin: None,
            welcome: View::new(String::new(), 1, 1),
            getting_started: None,
            tree: None,
            directory: None,
            sidebar: sidebar::Sidebar::new(sidebar),
            stubs: threads::stubs::StubState::from_config(&threads),
            inline_stubs: Vec::new(),
            expanded_layout_cache: Vec::new(),
            message_layout_cache: draw::message::MessageLayoutCache::default(),
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
            thread_store_path,
            store_backing,
            thread_store_error,
            thread_store_degraded: None,
            thread_watch_coverage: ThreadWatchCoverage::Covered,
            thread_updates_degraded: None,
            activity_store,
            activity_cursor,
            reach: Reach::everything(),
            record,
            dirs,
            toast_duration,
            highlighter,
            markdown,
            viewer,
            user,
            config_path,
            ignore,
            toasts: Vec::new(),
            review_points,
            comparison,
            comparison_status: Status::default(),
            diff_mode,
            last_active_diff_mode,
            off_target_paths: None,
            dormant_changed_filter: false,
            watching_root: true,
            status_stale: false,
            walks: status_walk::Walks::new(),
            status: Status::default(),
            worktrees: Vec::new(),
            worktree_paths: Vec::new(),
            reach_cache: HashMap::new(),
            elsewhere: HashMap::new(),
            local_thread_paths: HashMap::new(),
            rewatch: None,
        };
        if app.sidebar.tree && !app.ensure_tree() {
            app.sidebar.tree = false;
        }
        app.relayout();
        app.refresh_worktrees();
        app.refresh_status();
        app.refresh_comparison();
        app.refresh_reach();
        if shared_state_ancestor_count > 0 && app.message.is_none() {
            app.startup_warning(format!(
                "{shared_state_ancestor_count} group-writable state ancestor{}; run :doctor for details",
                if shared_state_ancestor_count == 1 {
                    ""
                } else {
                    "s"
                }
            ));
        }
        app
    }

    /// Recompute which threads `HEAD` shows (ADR 0024): one history walk
    /// for the commits the store mentions. Open threads a rewrite
    /// stranded follow `HEAD` first (ADR 0035). Marks are refreshed when
    /// the answer changed.
    pub(super) fn refresh_reach(&mut self) {
        if self.recompute_reach() {
            self.refresh_all_marks();
        }
    }

    /// Recompute reach without rebuilding document or tree projections.
    fn recompute_reach(&mut self) -> bool {
        let head = self.workspace.head_commit();
        let active = match (self.store.as_mut(), head) {
            (Some(store), Some(head)) => self
                .workspace
                .reachable(store.commits())
                .map(|reachable| (head, reachable)),
            _ => None,
        };
        self.reconcile_agent_activity();
        // The other worktrees widen the reach (ADR 0070).
        let scope = match active {
            Some((head, reachable)) => self.reach_with_others(head, reachable),
            None => Reach::everything(),
        };
        let changed = scope != self.reach;
        if changed {
            tracing::info!(head = ?self.workspace.head_commit(), "thread reach changed");
            self.reach = scope;
        }
        self.refresh_elsewhere();
        changed
    }

    /// Re-read the thread store after another writer appended to it: a
    /// second viewer, or a headless `--mcp` reply (ADR 0024). Marks and the
    /// expanded threads follow.
    pub(crate) fn reload_store(&mut self) -> StoreReload {
        self.observe_store_backing();
        let path = self.thread_store_path.clone();
        let before = match fs::symlink_metadata(&path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if self.store_backing != StoreBacking::InitiallyAbsent {
                    self.store_backing = StoreBacking::Missing;
                    let changed = self.set_thread_store_degraded(Some(format!(
                        "{} disappeared; keeping the last loaded board",
                        path.display()
                    )));
                    return StoreReload {
                        changed,
                        healthy: false,
                    };
                }
                if self.store.is_some() {
                    let changed = self.clear_store_degradation();
                    return StoreReload {
                        changed,
                        healthy: true,
                    };
                }
                None
            }
            Err(error) => {
                let changed = self.set_thread_store_degraded(Some(format!(
                    "cannot inspect {}: {error}; keeping the last loaded board",
                    path.display()
                )));
                return StoreReload {
                    changed,
                    healthy: false,
                };
            }
        };
        let store = if path == self.dirs.threads_file(self.record.key()) {
            Store::reload_workspace(&self.dirs, self.record.key())
        } else {
            Store::open(&path)
        };
        match store {
            Ok(store) => {
                if !stable_store_reload(before.as_ref(), &store, &path) {
                    let changed = self.set_thread_store_degraded(Some(format!(
                        "{} changed while it was loading; keeping the last loaded board",
                        path.display()
                    )));
                    return StoreReload {
                        changed,
                        healthy: false,
                    };
                }
                let changed = self.store.as_ref().is_none_or(|old| {
                    old.threads() != store.threads()
                        || old.archived_threads() != store.archived_threads()
                });
                if changed {
                    tracing::info!(threads = store.threads().len(), "thread store reloaded");
                }
                self.store_backing = if store.backing_file_observed() {
                    StoreBacking::Observed
                } else {
                    StoreBacking::InitiallyAbsent
                };
                self.store = Some(store);
                self.thread_store_error = None;
                let notice_changed = self.clear_store_degradation();
                let activity_changed = self.reconcile_agent_activity();
                if !changed {
                    return StoreReload {
                        changed: notice_changed || activity_changed,
                        healthy: true,
                    };
                }
                self.refresh_after_thread_store_change();
                StoreReload {
                    changed: true,
                    healthy: true,
                }
            }
            Err(error) => {
                tracing::warn!(%error, "cannot reload the thread store");
                let changed = self.set_thread_store_degraded(Some(format!(
                    "cannot reload {}: {error}; keeping the last loaded board",
                    path.display()
                )));
                StoreReload {
                    changed,
                    healthy: false,
                }
            }
        }
    }

    fn set_thread_updates_degraded(&mut self, reason: Option<String>) -> bool {
        if self.thread_updates_degraded == reason {
            return false;
        }
        let recovered = self.thread_updates_degraded.is_some() && reason.is_none();
        self.thread_updates_degraded = reason;
        if let Some(reason) = self.thread_updates_degraded.clone() {
            self.error(format!("thread updates degraded: {reason}; retrying"));
        } else if recovered {
            self.notice("thread updates restored");
        }
        true
    }

    pub(super) fn set_thread_store_degraded(&mut self, reason: Option<String>) -> bool {
        if self.thread_store_degraded == reason {
            return false;
        }
        self.thread_store_degraded = reason;
        let combined = self.thread_store_degraded.clone().or_else(|| {
            (self.thread_watch_coverage == ThreadWatchCoverage::Degraded)
                .then(|| THREAD_WATCH_DEGRADED.to_owned())
        });
        self.set_thread_updates_degraded(combined)
    }

    fn clear_store_degradation(&mut self) -> bool {
        self.set_thread_store_degraded(None)
    }

    pub(crate) fn thread_updates_healthy(&self) -> bool {
        self.thread_updates_degraded.is_none()
    }

    pub(crate) fn set_thread_watch_coverage(&mut self, covered: bool) -> bool {
        let coverage = if covered {
            ThreadWatchCoverage::Covered
        } else {
            ThreadWatchCoverage::Degraded
        };
        if self.thread_watch_coverage == coverage {
            return false;
        }
        self.thread_watch_coverage = coverage;
        let combined = self.thread_store_degraded.clone().or_else(|| {
            (self.thread_watch_coverage == ThreadWatchCoverage::Degraded)
                .then(|| THREAD_WATCH_DEGRADED.to_owned())
        });
        self.set_thread_updates_degraded(combined)
    }

    fn observe_store_backing(&mut self) {
        if self.store_backing == StoreBacking::InitiallyAbsent
            && self
                .store
                .as_ref()
                .is_some_and(Store::backing_file_observed)
        {
            self.store_backing = StoreBacking::Observed;
        }
    }

    /// Report newly observed agent messages from the append-only store.
    pub(super) fn reconcile_agent_activity(&mut self) -> bool {
        self.observe_store_backing();
        let Some(store) = self.store.as_ref() else {
            self.activity_store = None;
            self.activity_cursor = ActivityCursor::default();
            return false;
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
            return false;
        }
        if end < self.activity_cursor {
            tracing::warn!(
                path = %path.display(),
                observed = self.activity_cursor.ordinal(),
                current = end.ordinal(),
                "thread store activity cursor regressed; reseeding without replay"
            );
            self.activity_cursor = end;
            return false;
        }

        let activities = store
            .agent_activity_since(self.activity_cursor)
            .collect::<Vec<_>>();
        if activities.is_empty() {
            self.activity_cursor = end;
            return false;
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
        true
    }

    /// Where the thread store lives, for the watcher.
    pub(crate) fn store_path(&self) -> &Path {
        &self.thread_store_path
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
            self.notice("live updates degraded: workspace watch coverage is partial; retrying");
        }
        true
    }

    /// Live toasts, oldest first.
    pub(crate) fn toasts(&self) -> &[Toast] {
        &self.toasts
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
    /// raise transient toasts; the directories whose listings changed are
    /// re-read in the tree. A platform event-loss notice reconciles the
    /// whole remembered workspace from disk.
    #[expect(
        clippy::too_many_lines,
        reason = "watch batches keep event ordering and stale-state handling together"
    )]
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
        let mut observed_paths: HashSet<PathBuf> = HashSet::new();
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
                    if !observed_paths.insert(relative.clone()) {
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
        if git_changed || rules_changed || !changed.is_empty() {
            self.refresh_comparison();
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
        if events
            .iter()
            .any(|event| event.path() == Some(self.store_path()))
        {
            self.reload_store();
        }
    }

    /// `HEAD`, the index, a ref, or the worktree set moved: the worktrees
    /// are listed again (ADR 0070), the `HEAD` bases re-read, and the
    /// reach recomputed.
    fn on_git_changed(&mut self) {
        tracing::info!("git metadata changed; refreshing HEAD bases");
        self.refresh_worktrees();
        self.refresh_comparison();
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
        self.refresh_comparison();
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

    /// Root-relative `from` became `to`: a working-tree document follows,
    /// and thread paths are projected locally without rewriting the board.
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
        self.remember_thread_moves(&moved);
        self.refresh_review_paths();
        let tracks_working_tree = self.displayed_target_is_working_tree();
        if !tracks_working_tree {
            tracing::info!(
                from = %from.display(),
                to = %to.display(),
                "working rename left immutable comparison identities unchanged"
            );
            return;
        }
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
        for index in renamed {
            self.refresh_base(index);
            self.refresh_marks(index);
        }
        if let Some(target) = current_moved {
            self.notice(format!("renamed to {}", target.display()));
        }
    }

    /// The largest file the viewer reads (`viewer.max-file-size-mib`).
    pub(crate) fn max_file_bytes(&self) -> u64 {
        self.viewer.max_file_bytes()
    }

    /// The loaded text fingerprint used for in-memory rename pairing.
    ///
    /// The method name remains for the watcher/run seam owned elsewhere.
    pub(crate) fn loaded_fingerprint(&self, path: &Path) -> Option<Fingerprint> {
        let relative = path.strip_prefix(self.workspace.root()).ok()?;
        if let Some(text) = self
            .docs
            .iter()
            .find(|doc| doc.relative == relative)
            .and_then(|doc| doc.document.text())
        {
            return Some(Fingerprint::from_bytes(text.as_bytes()));
        }
        None
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
                let root = self.workspace.root().to_path_buf();
                let deleted_updates: Vec<(usize, Deleted, PathBuf)> =
                    if self.diff_mode == DiffMode::Off {
                        Vec::new()
                    } else {
                        self.docs
                            .iter()
                            .enumerate()
                            .filter_map(|(index, doc)| {
                                let missing = !root.join(&doc.relative).is_file();
                                let source = missing
                                    .then(|| deleted_source(self.status.get(&doc.relative)))
                                    .flatten()?;
                                Some((index, source, doc.relative.clone()))
                            })
                            .collect()
                    };
                let mut marks_to_refresh = Vec::new();
                for (index, source, relative) in deleted_updates {
                    let current = self.docs[index].deleted;
                    if current == Some(Deleted::ComparisonBase) {
                        continue;
                    }
                    if current.is_none() {
                        self.docs[index].deleted = Some(Deleted::Loaded);
                        self.docs[index].view.set_worktree_missing(true);
                    } else if current != Some(Deleted::Loaded) {
                        let bytes = match source {
                            Deleted::Index => self.workspace.index_bytes(&relative),
                            Deleted::Head => self.workspace.head_bytes(&relative),
                            Deleted::Loaded => unreachable!("loaded content is not a Git source"),
                            Deleted::ComparisonBase => continue,
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
                                            self.queue_highlight(index);
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
                self.refresh_comparison();
            }
            Err(error) => {
                self.status_stale = true;
                self.notice(format!("git status: {error}"));
            }
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
        let counts = match (loaded, diff) {
            (Some(_), Some(diff)) => Some(diff.counts()),
            _ => self.unloaded_change_counts(relative, absolute),
        };
        tracing::info!(path = %relative.display(), ?counts, "file edit observed");
        self.push_file_edit_toast(relative.to_path_buf(), counts.unwrap_or_default());
    }

    /// Counts for an unopened file against `HEAD`, within the content policy.
    fn unloaded_change_counts(
        &mut self,
        relative: &Path,
        absolute: &Path,
    ) -> Option<(usize, usize)> {
        if !self.workspace.is_git() {
            return None;
        }
        let policy = Policy {
            attr: self.workspace.diff_attr(relative),
            max_bytes: self.viewer.max_file_bytes(),
        };
        let current = match Document::load(absolute, policy) {
            Ok(document) => document.text().map(str::to_owned),
            Err(error) => {
                tracing::warn!(%error, "cannot count unopened file edit");
                None
            }
        }?;
        let base = match self.workspace.head_text_bounded(relative, policy.max_bytes) {
            Ok(Some(text)) if policy.attr.classify(text.as_bytes()) == Some(false) => text,
            Ok(Some(_) | None) => return None,
            Err(error) => {
                tracing::warn!(%error, "cannot read HEAD for unopened file edit");
                return None;
            }
        };
        Some(Diff::new(&base, &current).counts())
    }

    /// Expire transient toasts and the startup warning.
    pub(crate) fn tick(&mut self) {
        let now = Instant::now();
        self.toasts.retain(|toast| toast.until > now);
        if self
            .message
            .as_ref()
            .and_then(|notice| notice.until)
            .is_some_and(|until| until <= now)
        {
            self.message = None;
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
        if let Some(until) = self.message.as_ref().and_then(|notice| notice.until) {
            consider(until.saturating_duration_since(now));
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

    /// Rows reserved for the File surface header.
    pub(crate) fn file_chrome_rows(&self) -> usize {
        usize::from(
            (self.has_document() || self.directory_path().is_some())
                && !self.review_list().is_open()
                && !self.getting_started(),
        )
    }

    /// Rows left to the text once the banner and the file header are
    /// taken. The text's key bar takes none: it replaces the
    /// bottom text row while it has something to say (ADR 0067).
    pub(crate) fn text_rows(&self) -> usize {
        self.pane_rows()
            .saturating_sub(usize::from(self.banner().is_some()))
            .saturating_sub(self.file_chrome_rows())
            .max(1)
    }

    /// Whether the text's key bar replaces the bottom text row.
    pub(crate) fn text_bar_shown(&self) -> bool {
        self.has_document()
            && !self.getting_started()
            && self.directory_path().is_none()
            && !self.review_list().is_open()
            && self.info().is_none()
    }

    /// The screen row the text's key bar replaces: the bottom text row.
    pub(crate) fn text_bar_row(&self) -> usize {
        self.text_top() + self.text_rows() - 1
    }

    /// Rows over the text: the banner and the file header.
    pub(crate) fn text_top(&self) -> usize {
        self.pane_top() + usize::from(self.banner().is_some()) + self.file_chrome_rows()
    }

    pub(crate) fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.relayout();
        input::help::resize(self);
        doctor_view::resize(self);
        licenses::resize(self);
        menu_bar::resize(self);
    }

    fn relayout(&mut self) {
        let previous_width = self.view().layout().width();
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
        if self.view().layout().width() != previous_width {
            // Message and draft rows wrap at the effective text width.
            self.place_stub_rows();
        }
        self.scroll_tree();
    }

    /// The initial source width before a new document becomes `self.current`.
    fn document_width(&self, text: &str) -> usize {
        let gutter = draw::gutter_width_for_lines(LineIndex::new(text).line_count());
        self.width
            .saturating_sub(self.sidebar_width())
            .saturating_sub(gutter)
            .max(1)
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

    /// Raise a toast for `watch.toast`, dropping the oldest past the cap.
    pub(super) fn push_toast(&mut self, text: String) {
        self.push_toast_kind(text, ToastKind::Plain);
    }

    fn push_file_edit_toast(&mut self, path: PathBuf, counts: (usize, usize)) {
        let (added, removed) = counts;
        let text = match (added, removed) {
            (0, 0) => path.display().to_string(),
            (0, removed) => format!("{}  -{removed}", path.display()),
            (added, 0) => format!("{}  +{added}", path.display()),
            (added, removed) => format!("{}  +{added}  -{removed}", path.display()),
        };
        self.push_toast_kind(
            text,
            ToastKind::FileEdit {
                path,
                added,
                removed,
            },
        );
    }

    fn push_toast_kind(&mut self, text: String, kind: ToastKind) {
        if self.toast_duration == Duration::ZERO {
            return;
        }
        self.toasts.push(Toast {
            text,
            kind,
            until: Instant::now() + self.toast_duration,
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
            until: None,
        });
    }

    fn startup_warning(&mut self, message: impl Into<String>) {
        let message = message.into();
        tracing::warn!(message = %message, "status warning");
        self.message = Some(Notice {
            text: message,
            tone: NoticeTone::Warning,
            until: Some(Instant::now() + STARTUP_WARNING_DURATION),
        });
    }

    fn error(&mut self, message: impl Into<String>) {
        let message = message.into();
        tracing::error!(message = %message, "status error");
        self.message = Some(Notice {
            text: message,
            tone: NoticeTone::Error,
            until: None,
        });
    }

    /// Open the root-relative `path`, loading it or switching to it.
    #[expect(
        clippy::too_many_lines,
        reason = "opening retains the explicit endpoint and missing-path decisions together"
    )]
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
            let selected_target_is_working =
                self.comparison.target() == &ComparisonEndpoint::WorkingTree;
            let comparison_target_missing = self.diff_mode != DiffMode::Off
                && self
                    .comparison
                    .current()
                    .and_then(|comparison| {
                        comparison
                            .changes()
                            .iter()
                            .find(|change| change.path() == relative)
                    })
                    .is_some_and(|change| {
                        matches!(change.target(), fathomable_core::diff::PathState::Absent)
                    });
            let deleted = if self.diff_mode == DiffMode::Off {
                None
            } else if comparison_target_missing {
                Some(Deleted::ComparisonBase)
            } else {
                (selected_target_is_working && matches!(absolute.try_exists(), Ok(false)))
                    .then(|| deleted_source(self.status.get(&relative)))
                    .flatten()
            };
            let historical = (self.diff_mode != DiffMode::Off
                && (!selected_target_is_working || comparison_target_missing))
                .then(|| self.comparison_display_text(&relative));
            let mut target_absent = false;
            let document = if self.diff_mode == DiffMode::Off {
                self.off_target_document(&relative)
                    .map(|(document, absent)| {
                        target_absent = absent;
                        document
                    })
            } else {
                match historical {
                    Some(Ok(Some(text))) => {
                        Document::from_snapshot(&absolute, text.into_bytes(), policy)
                            .map_err(|error| error.to_string())
                    }
                    Some(Ok(None)) => Ok(Document::missing(&absolute, policy)),
                    Some(Err(error)) => Err(error),
                    None => match deleted {
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
                        Some(Deleted::Loaded) => {
                            unreachable!("new documents have no loaded snapshot")
                        }
                        Some(Deleted::ComparisonBase) => {
                            unreachable!("comparison-base content is handled above")
                        }
                        None => {
                            Document::load(&absolute, policy).map_err(|error| error.to_string())
                        }
                    },
                }
            };
            let mut comparison_notice = target_absent.then(|| "not present in Target".to_owned());
            let document = match document {
                Err(error)
                    if self.diff_mode != DiffMode::Off
                        && self.comparison_status.get(&relative).is_some() =>
                {
                    comparison_notice = Some(format!(
                        "content unavailable in selected comparison: {error}"
                    ));
                    Ok(Document::missing(&absolute, policy))
                }
                other => other,
            };
            match document {
                Ok(document) => {
                    // A binary or over-limit file has no text: the view is
                    // empty and the file-info pane draws instead (ADR 0026).
                    let text = document.text().unwrap_or_default().to_owned();
                    let width = self.document_width(&text);
                    let mut view = View::with_deferred_syntax(
                        text,
                        width,
                        self.pane_rows(),
                        self.syntax_for(&relative),
                    );
                    view.set_compare(self.comparison.compare());
                    view.set_worktree_missing(deleted.is_some());
                    view.set_index_missing(
                        self.status
                            .get(&relative)
                            .is_some_and(|entry| entry.staged_state() == Some(State::Deleted)),
                    );
                    if target_absent {
                        view.set_worktree_missing(true);
                    }
                    let id = self.highlights.next_document();
                    self.docs.push(Doc {
                        id,
                        document,
                        relative: relative.clone(),
                        view,
                        marks: Vec::new(),
                        draft: None,
                        deleted,
                        comparison_notice,
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

    fn show(&mut self, index: usize) {
        self.directory = None;
        if let Some(previous) = self.current
            && previous != index
        {
            self.park_draft();
        }
        self.current = Some(index);
        // A document takes the column back from the list (ADR 0025).
        self.close_review();
        self.focus = Focus::View;
        self.apply_comparison_projection(index);
        self.refresh_marks(index);
        if self.diff_mode == DiffMode::Unified {
            self.show_unified_diff();
        }
        // The document's stubs follow the session's toggles (ADR 0049).
        self.place_stub_rows();
        self.relayout();
        self.queue_highlight(index);
        self.resume_draft();
        tracing::info!(path = %self.current_path().display(), "showing document");
    }

    /// Where the reader is: an exact Reviews entry or a source line.
    fn position(&self) -> Option<jumplist::Position> {
        if self.review_list().is_open()
            && let Some(thread) = self.thread_cursor().thread().cloned()
        {
            return Some(jumplist::Position::Review { thread });
        }
        let path = self.current_path().to_path_buf();
        if path.as_os_str().is_empty() {
            return None;
        }
        let line = self.view().cursor_source_line().unwrap_or(1);
        let thread = self
            .thread_cursor()
            .thread()
            .filter(|id| self.threads_at_cursor().contains(*id))
            .cloned();
        Some(jumplist::Position::File { path, line, thread })
    }

    /// A far move is leaving `from` (ADR 0049).
    pub(crate) fn record_jump(&mut self, from: jumplist::Position) {
        self.jumplist.record(from);
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
        match target {
            jumplist::Position::File { path, line, thread } => {
                self.open_file_view();
                self.focus = Focus::View;
                if self.current_path() != path {
                    self.open(path);
                    if self.current_path() != path {
                        return;
                    }
                }
                self.view_mut().goto_source_line(*line);
                if let Some(thread) = thread
                    && self.marks().iter().any(|mark| mark.id() == thread)
                {
                    self.set_thread_cursor(thread.clone());
                }
            }
            jumplist::Position::Review { thread } => {
                self.show_thread_in_review(thread);
            }
        }
    }

    /// Re-read the working document at `index`.
    ///
    /// The returned diff is between consecutive working-tree contents for
    /// change hints. Persistent live relocation applies only while the
    /// effective comparison target is that working tree; immutable historical
    /// displays are projected again without rewriting thread placement.
    fn reload_doc(&mut self, index: usize) -> Option<Diff> {
        let old = self.docs[index]
            .document
            .text()
            .unwrap_or_default()
            .to_owned();
        let tracks_working_tree = self.displayed_target_is_working_tree();
        match self.docs[index].document.reload() {
            Ok(true) => {
                let doc = &mut self.docs[index];
                tracing::info!(path = %doc.relative.display(), "reloaded after change");
                let text = doc.document.text().unwrap_or_default();
                let diff = Diff::new(&old, text);
                doc.view.reload(text.to_owned());
                self.apply_comparison_projection(index);
                self.queue_highlight(index);
                if tracks_working_tree {
                    self.remap_marks(index, &old);
                } else {
                    self.refresh_marks(index);
                }
                Some(diff)
            }
            Ok(false) => None,
            Err(error) => {
                tracing::warn!(%error, "reload failed; keeping previous text");
                None
            }
        }
    }

    /// Re-apply the selected comparison projection to a loaded document.
    fn refresh_base(&mut self, index: usize) {
        self.apply_comparison_projection(index);
    }

    // ----- sidebar -----

    fn ensure_tree(&mut self) -> bool {
        if self.tree.is_some() {
            return true;
        }
        match Tree::new(&mut self.workspace) {
            Ok(mut tree) => {
                let status = self.comparison_status().clone();
                tree.sift(&status);
                if let Some(paths) = self.comparison_snapshot_paths() {
                    tree.set_snapshot_paths(&status, paths);
                } else {
                    tree.set_virtual_paths(&status, self.comparison_virtual_paths());
                }
                self.tree = Some(tree);
                self.refresh_review_paths();
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

    /// Open the confirmation required by bare `q`.
    pub(crate) fn request_quit(&mut self) {
        self.popup = Some(Popup::ConfirmQuit);
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
            PickerKind::ComparisonBase => self.comparison_choices(false),
            PickerKind::ComparisonTarget => self.comparison_choices(true),
            PickerKind::ComparisonTags(_) => self.comparison_tag_choices(),
            PickerKind::ComparisonBranches(_) => self.comparison_branch_choices(),
            PickerKind::ComparisonBranchCommits(_) => Vec::new(),
            PickerKind::ComparisonReviewPoints => self.comparison_review_point_choices(),
            PickerKind::ComparisonAdvanced(_) => vec!["Empty tree".to_owned()],
            PickerKind::ReviewPointName => vec!["save without a name".to_owned()],
            PickerKind::Worktree => self.worktree_choices(),
        };
        self.open_scoped_picker(kind, items, None);
    }

    pub(crate) fn open_scoped_picker(
        &mut self,
        kind: PickerKind,
        items: Vec<String>,
        scope: Option<String>,
    ) {
        tracing::info!(?kind, items = items.len(), "picker opened");
        self.park_draft();
        self.popup = Some(Popup::Picker(PickerState::scoped(kind, items, scope)));
    }

    fn index(&mut self, filter: Filter) -> Vec<String> {
        if let Some(paths) = self.comparison_snapshot_paths() {
            return paths
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
        }
        let mut files = match filter {
            Filter::All => self.all_index.files(&mut self.workspace),
            Filter::Visible => self.file_index.files(&mut self.workspace),
        };
        if self.diff_mode != DiffMode::Off {
            for path in self
                .comparison()
                .into_iter()
                .flat_map(fathomable_core::diff::Comparison::changes)
                .map(|change| change.path().to_string_lossy().into_owned())
            {
                if !files.contains(&path) {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }

    fn picker_mut(&mut self) -> Option<&mut PickerState> {
        match self.popup.as_mut() {
            Some(Popup::Picker(state)) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn picker_char(&mut self, ch: char) {
        let search = if let Some(picker) = self.picker_mut() {
            picker.input.push(ch);
            picker.commit_search_request()
        } else {
            None
        };
        if let Some(search) = search {
            self.complete_picker_commit_search(&search);
        }
    }

    pub(crate) fn picker_backspace(&mut self) {
        let search = if let Some(picker) = self.picker_mut() {
            picker.input.pop();
            picker.commit_search_request()
        } else {
            None
        };
        if let Some(search) = search {
            self.complete_picker_commit_search(&search);
        }
    }

    pub(crate) fn picker_move(&mut self, delta: isize) {
        let rows = picker_list_rows(self.pane_rows());
        if let Some(picker) = self.picker_mut() {
            picker.move_by(delta, rows);
        }
    }

    pub(crate) fn picker_select(&mut self, index: usize) {
        let rows = picker_list_rows(self.pane_rows());
        if let Some(picker) = self.picker_mut()
            && index < picker.matches.len()
        {
            picker.scroll = picker.first_visible(rows);
            picker.selected = index;
            picker.scroll = picker.first_visible(rows);
        }
    }

    pub(crate) fn picker_escape(&mut self) {
        let parent = self.picker_mut().map(|picker| match picker.kind {
            PickerKind::ComparisonBranchCommits(side) => Some(PickerKind::ComparisonBranches(side)),
            PickerKind::ComparisonTags(side)
            | PickerKind::ComparisonBranches(side)
            | PickerKind::ComparisonAdvanced(side) => Some(side.picker_kind()),
            PickerKind::ComparisonReviewPoints => Some(PickerKind::ComparisonBase),
            _ => None,
        });
        match parent.flatten() {
            Some(kind) => self.open_picker(kind),
            None => self.close_popup(),
        }
    }

    /// Enter in the picker: open the file, or show the thread.
    pub(crate) fn picker_confirm(&mut self) {
        let choice = self.picker_mut().and_then(|picker| {
            if let Some(m) = picker.matches.get(picker.selected) {
                Some((
                    picker.kind,
                    picker.item(m).to_owned(),
                    picker.input().to_owned(),
                ))
            } else if matches!(
                picker.kind,
                PickerKind::ComparisonBase
                    | PickerKind::ComparisonTarget
                    | PickerKind::ComparisonBranchCommits(_)
                    | PickerKind::ReviewPointName
            ) && !picker.input().trim().is_empty()
            {
                Some((
                    picker.kind,
                    picker.input().to_owned(),
                    picker.input().to_owned(),
                ))
            } else {
                None
            }
        });
        self.popup = None;
        match choice {
            Some((PickerKind::Files | PickerKind::AllFiles | PickerKind::Recent, path, _)) => {
                self.open(Path::new(&path));
            }
            Some((
                kind @ (PickerKind::ComparisonBase | PickerKind::ComparisonTarget),
                item,
                input,
            )) => {
                self.choose_diff_side_input(kind, &item, &input);
            }
            Some((
                kind @ (PickerKind::ComparisonTags(_)
                | PickerKind::ComparisonBranches(_)
                | PickerKind::ComparisonBranchCommits(_)
                | PickerKind::ComparisonReviewPoints
                | PickerKind::ComparisonAdvanced(_)),
                item,
                input,
            )) => {
                self.choose_nested_comparison(kind, &item, &input);
            }
            Some((PickerKind::ReviewPointName, item, input)) => {
                let name = if input.trim().is_empty() {
                    None
                } else {
                    Some(input.trim())
                };
                let _ = item;
                self.save_review_point(name);
            }
            Some((PickerKind::Worktree, item, _)) => self.choose_worktree(&item),
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
    /// The thread-store startup failure shown when `store` is unavailable.
    pub(crate) thread_store_error: Option<StoreError>,
    /// File-watcher settings (ADR 0015).
    pub(crate) watch: WatchConfig,
    /// Explicit workspace review points, or `None` when storage is unavailable.
    pub(crate) review_points: Option<fathomable_core::review_points::ReviewPointStore>,
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
    /// Shared external state ancestors found once during startup.
    pub(crate) shared_state_ancestor_count: usize,
}

#[cfg(test)]
impl Options {
    /// Options for a test app rooted at `root`: no stores, plain code.
    pub(crate) fn for_test(root: PathBuf) -> Self {
        use fathomable_core::session::Id;
        Self {
            record: Record::new(Id::mint(), root.clone(), root),
            // Isolate each test process from older runs' state and permissions.
            dirs: XdgDirs::resolve(|name| {
                (name == "XDG_STATE_HOME").then(|| {
                    std::env::temp_dir()
                        .join("fathomable-test-state")
                        .join(std::process::id().to_string())
                        .into()
                })
            }),
            store: None,
            thread_store_error: None,
            watch: WatchConfig::default(),
            review_points: None,
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
            shared_state_ancestor_count: 0,
        }
    }
}

#[cfg(test)]
mod render_integrity_tests;
#[cfg(test)]
mod tests;
