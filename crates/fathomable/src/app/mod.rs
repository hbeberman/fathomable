// @okf-doc: /decisions/0012-workspace-mode.md
//! The workspace app: open documents, sidebar, popups, event loop, socket.
//!
//! [`App`] is plain state so ADR 0012 behaviour can be tested without a
//! terminal; `ui` draws it, `keys` drives it, and [`run`] owns the terminal,
//! the file watcher, and the session socket.

mod clipboard;
mod commands;
mod keys;
mod reanchor;
mod socket;
mod threads;
mod ui;
mod view;

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fathomable_core::Document;
use fathomable_core::annotations::{Store, ThreadId};
use fathomable_core::config::{FollowConfig, MarkdownConfig};
use fathomable_core::diff::Diff;
use fathomable_core::editor::Cell;
use fathomable_core::follow::{Change, Delta, Ignore, Queue, Target};
use fathomable_core::highlight::Highlighter;
use fathomable_core::picker::{Match, Picker};
use fathomable_core::seen;
use fathomable_core::session::{FollowState, Record, Request, Response};
use fathomable_core::status::Status;
use fathomable_core::theme::Theme;
use fathomable_core::tree::{Activation, Tree};
use fathomable_core::workspace::{EntryKind, Filter, Workspace};
use notify::{RecursiveMode, Watcher};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

pub use threads::{Compose, Mark, ThreadPanel};
use view::{Effect, HunkStep, Syntax, View};

/// How long to wait after a change notification before re-reading, so an
/// editor's write-then-rename lands as one reload.
/// Reader activity newer than this holds auto-jump back (ADR 0015).
const RECENT_ACTIVITY: Duration = Duration::from_secs(3);

/// How many edit deltas a document keeps.
const MAX_DELTAS: usize = 8;

/// Toasts visible at once.
pub const MAX_TOASTS: usize = 3;

/// Sidebar width in columns before clamping to a third of the terminal.
const SIDEBAR_WIDTH: usize = 32;

/// Narrowest the tree can be dragged.
const SIDEBAR_MIN_WIDTH: usize = 8;

/// Fewest text columns a drag leaves the view.
const TEXT_MIN_WIDTH: usize = 20;

/// Shortest the thread pane can be dragged: rule, header, one body row.
const THREAD_MIN_ROWS: usize = 3;
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
    /// The thread pane (ADR 0013).
    Thread,
}

/// A pane border the mouse is dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Border {
    /// The rule between the tree and the text.
    Sidebar,
    /// The rule along the top of the thread pane.
    Thread,
    /// The rule along the top of the comment box (ADR 0018).
    Compose,
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
    /// Threads on the current document (ADR 0013).
    Threads,
}

/// The open picker popup.
#[derive(Debug)]
pub struct PickerState {
    kind: PickerKind,
    picker: Picker,
    input: String,
    matches: Vec<Match>,
    selected: usize,
    /// Thread behind each item, for [`PickerKind::Threads`].
    ids: Vec<ThreadId>,
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
            ids: Vec::new(),
        }
    }

    fn with_ids(mut self, ids: Vec<ThreadId>) -> Self {
        self.ids = ids;
        self
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
    /// The Helix-style `Space` menu.
    Space,
    /// Every key binding.
    Help,
    /// A file, recent-document, or thread picker.
    Picker(PickerState),
    /// The comment box (ADR 0013).
    Compose(Compose),
    /// The `Space j` follow submenu (ADR 0015).
    Jump,
    /// The `:status` overlay (ADR 0021).
    Status,
}

/// One space-menu entry: key, label.
pub const SPACE_MENU: [(char, &str); 9] = [
    ('e', "toggle tree focus"),
    ('E', "hide tree"),
    ('f', "open file"),
    ('F', "open file (incl. ignored)"),
    ('o', "recent files"),
    ('a', "thread at cursor"),
    ('A', "threads in file"),
    ('j', "follow / jump"),
    ('?', "all keys"),
];

/// The `Space j` submenu: key, label.
pub const JUMP_MENU: [(char, &str); 3] = [
    ('j', "jump to newest change"),
    ('a', "toggle auto-jump"),
    ('c', "clear changes"),
];

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

/// Every binding, for `Space ?`.
pub const HELP: [(&str, &str); 36] = [
    ("j / k", "move down / up"),
    ("h / l", "move left / right"),
    ("gg / G", "top / bottom"),
    ("Ctrl-d / Ctrl-u", "half page down / up"),
    ("/ ?", "search forward / backward"),
    ("n / N", "next / previous match"),
    ("v / V / mouse drag", "select text / lines / cells"),
    ("y (selected)", "copy source to clipboard"),
    ("c (selected)", "comment on the selection"),
    ("Space a", "read thread at cursor"),
    ("Space A", "pick a thread in this file"),
    ("]c / [c", "next / previous thread"),
    ("thread r x n p j k", "reply, resolve, switch, scroll"),
    ("comment Enter", "newline; Ctrl-Enter or Alt-Enter submits"),
    ("gs / :source", "toggle source view"),
    ("gd / :diff", "toggle the diff against HEAD"),
    ("gD / :diff seen", "toggle the diff against last seen"),
    ("]g / [g", "next / previous hunk, across files"),
    ("]G / [G", "next / previous uncommitted file"),
    ("]f / [f", "next / previous changed file"),
    ("Space j", "follow: jump, auto, clear"),
    (":follow", "toggle auto-jump"),
    (":status", "session, paths, follow state"),
    ("[o / ]o", "previous / next opened file"),
    (":N", "go to source line N"),
    (":noh", "clear search highlight"),
    (":q", "quit"),
    ("Space e", "tree: open and focus, or return focus"),
    ("Space E", "tree: hide"),
    ("Space f / F", "file picker / including ignored"),
    ("Space o", "recent files"),
    ("tree j k h l Enter", "move, collapse, expand or open"),
    ("tree R", "re-read directories"),
    ("tree I", "show ignored entries"),
    ("picker Up Down Ctrl-n Ctrl-p", "move selection"),
    ("Esc", "close / clear"),
];

#[derive(Debug)]
struct Doc {
    document: Document,
    relative: PathBuf,
    view: View,
    marks: Vec<Mark>,
    /// What recent reloads changed, newest last (ADR 0015 edit deltas).
    deltas: Vec<Delta>,
    /// Whether the text has changed or been read since it was last
    /// snapshotted as seen.
    seen_dirty: bool,
}

/// All application state.
#[derive(Debug)]
pub struct App {
    workspace: Workspace,
    docs: Vec<Doc>,
    current: Option<usize>,
    history: Vec<usize>,
    history_pos: usize,
    welcome: View,
    tree: Option<Tree>,
    sidebar_visible: bool,
    sidebar_scroll: usize,
    /// Tree width once dragged; the default follows the terminal.
    sidebar_cols: Option<usize>,
    /// The thread pane along the bottom of the text (ADR 0013).
    thread: Option<ThreadPanel>,
    /// Thread pane height once dragged; the default follows the terminal.
    thread_rows: Option<usize>,
    /// Comment box height once dragged; the default follows its text.
    compose_rows: Option<usize>,
    /// The border a mouse drag is moving.
    drag: Option<Border>,
    focus: Focus,
    popup: Option<Popup>,
    file_index: Option<Vec<String>>,
    all_index: Option<Vec<String>>,
    message: Option<String>,
    pending: Option<char>,
    width: usize,
    height: usize,
    session: String,
    store: Option<Store>,
    /// Files an agent said it is working on (ADR 0014 `follow`).
    followed: Vec<PathBuf>,
    record: Record,
    follow: FollowConfig,
    /// Code highlighting shared by every view (ADR 0016).
    highlighter: Arc<Highlighter>,
    /// Which files render as Markdown (ADR 0016).
    markdown: MarkdownConfig,
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
            store,
            follow,
            seen,
            highlighter,
            markdown,
        } = options;
        let ignore = match Ignore::new(&follow.ignore) {
            Ok(ignore) => ignore,
            Err(error) => {
                tracing::warn!(%error, "ignoring follow.ignore");
                Ignore::default()
            }
        };
        let mut app = Self {
            workspace,
            docs: Vec::new(),
            current: None,
            history: Vec::new(),
            history_pos: 0,
            welcome: View::new(String::new(), 1, 1),
            tree: None,
            sidebar_visible: false,
            sidebar_scroll: 0,
            sidebar_cols: None,
            thread: None,
            thread_rows: None,
            compose_rows: None,
            drag: None,
            focus: Focus::View,
            popup: None,
            file_index: None,
            all_index: None,
            message: None,
            pending: None,
            width,
            height,
            session: record.id().to_string(),
            store,
            followed: Vec::new(),
            record,
            auto: follow.auto,
            follow,
            highlighter,
            markdown,
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
        app.reanchor_from_snapshots();
        app
    }

    /// How a root-relative `path` should be coloured and first displayed.
    fn syntax_for(&self, path: &Path) -> Syntax {
        Syntax {
            highlighter: Arc::clone(&self.highlighter),
            hint: path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default(),
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

    /// Toggle auto-jump (`Space j a`, `:follow`).
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

    /// Set auto-jump (`:follow on` / `:follow off`).
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

    /// Files the watcher reported, absolute. Loaded documents reload;
    /// changes that pass the source and ignore rules join the queue.
    pub fn on_changes(&mut self, paths: Vec<PathBuf>) {
        let root = self.workspace.root().to_path_buf();
        let mut git_changed = false;
        let mut seen_paths: HashSet<PathBuf> = HashSet::new();
        for absolute in paths {
            let Ok(relative) = absolute.strip_prefix(&root) else {
                continue;
            };
            if relative.starts_with(".git") {
                git_changed |= is_git_metadata(relative);
                continue;
            }
            if !seen_paths.insert(relative.to_path_buf()) {
                continue;
            }
            self.on_change(relative, &absolute);
        }
        if git_changed {
            tracing::info!("git metadata changed; refreshing HEAD bases");
            for index in 0..self.docs.len() {
                self.refresh_base(index);
            }
        }
        if git_changed || !seen_paths.is_empty() {
            self.refresh_status();
        }
        // A file appearing or vanishing changes some directory's listing;
        // ignored paths (a build under `target/`) never reach the tree, so
        // they cost nothing here.
        let tree_dirty = seen_paths.iter().any(|relative| {
            !(self.workspace.is_ignored(relative, EntryKind::File)
                || self.ignore.is_ignored(relative))
        });
        if tree_dirty {
            self.file_index = None;
            self.all_index = None;
            self.with_tree_result(|tree, workspace| tree.refresh(workspace).map(|()| None));
        }
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
        let delta = loaded.and_then(|index| self.reload_doc(index));
        if loaded.is_some() && delta.is_none() {
            // The event did not change the text (a touch, or our own
            // write); nothing to hint about.
            return;
        }
        if self.workspace.is_ignored(relative, EntryKind::File) || self.ignore.is_ignored(relative)
        {
            return;
        }
        let (line, counts) = match (loaded, delta) {
            (Some(index), Some(delta)) => (
                delta
                    .first_line()
                    .or_else(|| self.docs[index].view.first_hunk_line()),
                delta.diff().counts(),
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
        if self.follow.toast > Duration::ZERO {
            let (added, removed) = counts;
            let text = if added == 0 && removed == 0 {
                change.path.display().to_string()
            } else {
                format!("{} +{added} -{removed}", change.path.display())
            };
            self.toasts.push(Toast {
                text,
                until: Instant::now() + self.follow.toast,
            });
            if self.toasts.len() > MAX_TOASTS {
                self.toasts.remove(0);
            }
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
            && self.docs[index].view.idle() >= self.follow.seen_idle
        {
            self.mark_seen(index);
        }
        if self.auto
            && let Some(change) = self.queue.newest().cloned()
            && self
                .last_change
                .is_some_and(|at| at.elapsed() >= self.follow.jump_debounce)
            && self.auto_jump_allowed()
        {
            self.jump_to(&change);
        }
    }

    fn auto_jump_allowed(&self) -> bool {
        if self.popup.is_some() || self.thread.is_some() {
            return false;
        }
        let Some(index) = self.current else {
            return true;
        };
        let view = &self.docs[index].view;
        view.selection().is_none()
            && view.mode() == view::Mode::Normal
            && !view.diff_view()
            && view.idle() >= RECENT_ACTIVITY
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
            consider(self.follow.seen_idle.saturating_sub(idle));
        }
        if self.auto && !self.queue.is_empty() {
            let since = self.last_change.map_or(Duration::ZERO, |at| at.elapsed());
            let wait = self.follow.jump_debounce.saturating_sub(since);
            let activity = self.current.map_or(Duration::ZERO, |i| {
                RECENT_ACTIVITY.saturating_sub(self.docs[i].view.idle())
            });
            consider(wait.max(activity).max(Duration::from_millis(50)));
        }
        next
    }

    /// Snapshot the document at `index` as seen.
    pub(super) fn mark_seen(&mut self, index: usize) {
        let Some(doc) = self.docs.get_mut(index) else {
            return;
        };
        doc.seen_dirty = false;
        let Some(seen) = self.seen.as_mut() else {
            return;
        };
        match seen.record(&doc.relative, doc.document.text()) {
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

    /// The session id, as `--sessions` prints it.
    pub fn session(&self) -> &str {
        &self.session
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
            Focus::Thread => self.thread.is_some(),
        };
        if present {
            self.focus = focus;
        }
    }

    /// Whether a document is open, rather than the welcome screen.
    pub fn has_document(&self) -> bool {
        self.current.is_some()
    }

    /// The open thread pane.
    pub fn thread_panel(&self) -> Option<&ThreadPanel> {
        self.thread.as_ref()
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
            Some(Border::Thread) => {
                self.thread_rows = Some(
                    self.pane_rows()
                        .saturating_sub(self.compose_rows())
                        .saturating_sub(row),
                );
            }
            Some(Border::Compose) => self.compose_rows = Some(self.pane_rows().saturating_sub(row)),
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
        self.tree.as_ref().filter(|_| self.sidebar_visible)
    }

    pub fn sidebar_scroll(&self) -> usize {
        self.sidebar_scroll
    }

    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    pub fn pending(&self) -> Option<char> {
        self.pending.or_else(|| self.view().pending())
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

    /// Sidebar width in columns, 0 when hidden.
    pub fn sidebar_width(&self) -> usize {
        if !self.sidebar_visible {
            return 0;
        }
        let widest = self.width.saturating_sub(TEXT_MIN_WIDTH);
        self.sidebar_cols
            .map_or_else(
                || SIDEBAR_WIDTH.min(self.width / 3),
                |cols| cols.min(widest),
            )
            .max(SIDEBAR_MIN_WIDTH)
    }

    /// Rows available to panes once the status line is taken.
    pub fn pane_rows(&self) -> usize {
        self.height.saturating_sub(1).max(1)
    }

    /// Rows the thread pane takes along the bottom, 0 when closed.
    pub fn thread_rows(&self) -> usize {
        if self.thread.is_none() {
            return 0;
        }
        let rows = self.pane_rows();
        let tallest = rows.saturating_sub(1);
        self.thread_rows
            .unwrap_or_else(|| (rows / 3).max(6))
            .clamp(THREAD_MIN_ROWS.min(tallest), tallest)
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
            .saturating_sub(self.sidebar_width())
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
        self.pane_rows().saturating_sub(self.thread_rows()).max(1)
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.relayout();
    }

    fn relayout(&mut self) {
        let rows = self.text_rows();
        let sidebar = self.sidebar_width();
        let text_width = self
            .width
            .saturating_sub(sidebar)
            .saturating_sub(ui::gutter_width(self.view()))
            .max(1);
        self.view_mut().resize(text_width, rows);
        self.scroll_sidebar();
    }

    pub fn clear_message(&mut self) {
        self.message = None;
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
            match Document::load(&absolute) {
                Ok(document) => {
                    let view = View::with_syntax(
                        document.text().to_owned(),
                        1,
                        1,
                        self.syntax_for(&relative),
                    );
                    self.docs.push(Doc {
                        document,
                        relative: relative.clone(),
                        view,
                        marks: Vec::new(),
                        deltas: Vec::new(),
                        seen_dirty: true,
                    });
                    self.docs.len() - 1
                }
                Err(error) => {
                    self.notice(format!("{error:#}"));
                    return;
                }
            }
        };
        self.history.truncate(self.history_pos);
        self.history.push(index);
        self.history_pos = self.history.len();
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
                Response::Done
            }
            Request::AnnotationsList { since, path } => match &self.store {
                Some(store) => Response::Threads(
                    store
                        .threads()
                        .iter()
                        .filter(|t| since.is_none_or(|s| t.updated() >= s))
                        .filter(|t| path.as_deref().is_none_or(|p| t.path() == p))
                        .cloned()
                        .collect(),
                ),
                None => Response::Error("annotations unavailable; see the log".to_owned()),
            },
            Request::ThreadReply {
                thread,
                author,
                body,
                resolve,
            } => match self.agent_reply(&thread, author, body, resolve) {
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
        if let Some(line) = line {
            let view = self.view_mut();
            view.escape();
            view.goto_source_line(line);
            if let Some(end) = end_line.filter(|end| *end > line) {
                view.select_lines();
                view.goto_source_line(end);
            }
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
        self.focus = Focus::View;
        self.refresh_base(index);
        self.refresh_marks(index);
        self.relayout();
        tracing::info!(path = %self.current_path().display(), "showing document");
    }

    /// `[o`: the previously opened document.
    pub fn history_back(&mut self) {
        if self.history_pos > 1 {
            self.history_pos -= 1;
            self.show(self.history[self.history_pos - 1]);
        } else {
            self.notice("at oldest file");
        }
    }

    /// `]o`: the next document in history.
    pub fn history_forward(&mut self) {
        if self.history_pos < self.history.len() {
            self.history_pos += 1;
            self.show(self.history[self.history_pos - 1]);
        } else {
            self.notice("at newest file");
        }
    }

    /// Re-read the document at `index`; the delta of what changed, if the
    /// text differs.
    fn reload_doc(&mut self, index: usize) -> Option<Delta> {
        let doc = &mut self.docs[index];
        match doc.document.reload() {
            Ok(true) => {
                tracing::info!(path = %doc.relative.display(), "reloaded after change");
                let old = doc.view.text().to_owned();
                let delta = Delta::new(old, doc.document.text());
                doc.view.reload(doc.document.text().to_owned());
                doc.deltas.push(delta.clone());
                if doc.deltas.len() > MAX_DELTAS {
                    doc.deltas.remove(0);
                }
                doc.seen_dirty = true;
                self.refresh_base(index);
                self.remap_marks(index, delta.old());
                Some(delta)
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
        let Some(relative) = self.docs.get(index).map(|doc| doc.relative.clone()) else {
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

    pub fn set_pending(&mut self, key: Option<char>) {
        self.pending = key;
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

    /// Show and focus the tree, as a workspace start does (ADR 0012).
    pub fn show_sidebar(&mut self) {
        if !self.sidebar_visible {
            self.toggle_sidebar_focus();
        }
    }

    /// `Space e`: open and focus the tree, or hand focus back.
    pub fn toggle_sidebar_focus(&mut self) {
        if !self.sidebar_visible {
            if !self.ensure_tree() {
                return;
            }
            self.sidebar_visible = true;
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

    /// `Space E`: hide the tree.
    pub fn hide_sidebar(&mut self) {
        self.sidebar_visible = false;
        self.focus = Focus::View;
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

    /// Run `f` on the tree, then keep the cursor on screen.
    pub fn with_tree(&mut self, f: impl FnOnce(&mut Tree, &mut Workspace) -> Option<Activation>) {
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let activation = f(tree, &mut self.workspace);
        self.scroll_sidebar();
        if let Some(Activation::Open(path)) = activation {
            self.open(&path);
        }
    }

    /// A tree operation that can fail: report the error on the status line.
    pub fn with_tree_result(
        &mut self,
        f: impl FnOnce(
            &mut Tree,
            &mut Workspace,
        )
            -> Result<Option<Activation>, fathomable_core::workspace::WorkspaceError>,
    ) {
        let mut failure = None;
        self.with_tree(|tree, workspace| match f(tree, workspace) {
            Ok(activation) => activation,
            Err(error) => {
                failure = Some(error.to_string());
                None
            }
        });
        if let Some(message) = failure {
            self.notice(message);
        }
    }

    /// `R` in the tree: re-read directories and drop the picker indexes.
    pub fn refresh_tree(&mut self) {
        self.file_index = None;
        self.all_index = None;
        self.with_tree_result(|tree, workspace| tree.refresh(workspace).map(|()| None));
        self.notice("tree refreshed");
    }

    /// `I` in the tree: toggle ignored entries.
    pub fn toggle_ignored(&mut self) {
        let filter = match self.tree.as_ref().map(Tree::filter) {
            Some(Filter::Visible) => Filter::All,
            _ => Filter::Visible,
        };
        self.with_tree_result(move |tree, workspace| {
            tree.set_filter(workspace, filter).map(|()| None)
        });
    }

    /// A click on sidebar row `row` (screen coordinates).
    pub fn sidebar_click(&mut self, row: usize) {
        let index = self.sidebar_scroll + row;
        self.focus = Focus::Sidebar;
        self.with_tree_result(|tree, workspace| {
            if index >= tree.rows().len() {
                return Ok(None);
            }
            tree.set_cursor(index);
            tree.activate(workspace)
        });
    }

    fn scroll_sidebar(&mut self) {
        let rows = self.pane_rows().saturating_sub(1).max(1);
        let Some(tree) = self.tree.as_ref() else {
            return;
        };
        let cursor = tree.cursor();
        let off = SIDEBAR_SCROLLOFF.min(rows.saturating_sub(1) / 2);
        if cursor < self.sidebar_scroll + off {
            self.sidebar_scroll = cursor.saturating_sub(off);
        }
        if cursor + off >= self.sidebar_scroll + rows {
            self.sidebar_scroll = (cursor + off + 1).saturating_sub(rows);
        }
        let max = tree.rows().len().saturating_sub(rows);
        self.sidebar_scroll = self.sidebar_scroll.min(max);
    }

    // ----- popups -----

    pub fn open_space_menu(&mut self) {
        self.popup = Some(Popup::Space);
    }

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

    /// Run a space-menu entry; unknown keys just close the menu.
    pub fn space_menu_select(&mut self, key: char) {
        self.popup = None;
        match key {
            'e' => self.toggle_sidebar_focus(),
            'E' => self.hide_sidebar(),
            'f' => self.open_picker(PickerKind::Files),
            'F' => self.open_picker(PickerKind::AllFiles),
            'o' => self.open_picker(PickerKind::Recent),
            'a' => self.open_thread_at_cursor(),
            'A' => self.open_thread_picker(),
            'j' => self.popup = Some(Popup::Jump),
            '?' => self.open_help(),
            _ => {}
        }
    }

    /// Run a `Space j` entry; unknown keys just close the menu.
    pub fn jump_menu_select(&mut self, key: char) {
        self.popup = None;
        match key {
            'j' => self.jump_newest(),
            'a' => self.toggle_auto_jump(),
            'c' => self.clear_queue(),
            _ => {}
        }
    }

    pub fn open_picker(&mut self, kind: PickerKind) {
        let items = match kind {
            PickerKind::Files => self.index(Filter::Visible),
            PickerKind::AllFiles => self.index(Filter::All),
            PickerKind::Recent => {
                let mut seen = Vec::new();
                for &index in self.history[..self.history_pos].iter().rev() {
                    let path = self.docs[index].relative.to_string_lossy().into_owned();
                    if !seen.contains(&path) {
                        seen.push(path);
                    }
                }
                seen
            }
            PickerKind::Threads => {
                self.open_thread_picker();
                return;
            }
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
            let id = picker.ids.get(m.index()).cloned();
            Some((picker.kind, picker.item(m).to_owned(), id))
        });
        self.popup = None;
        match choice {
            Some((PickerKind::Threads, _, Some(id))) => self.show_thread(id),
            Some((PickerKind::Files | PickerKind::AllFiles | PickerKind::Recent, path, _)) => {
                self.open(Path::new(&path));
            }
            _ => {}
        }
    }
}

/// Everything [`App::new`] needs beyond the workspace and terminal size.
#[derive(Debug)]
pub struct Options {
    /// This session's record, already written to the sessions directory.
    pub record: Record,
    /// The workspace's thread store, or `None` when it could not be opened.
    pub store: Option<Store>,
    /// Follow-mode settings (ADR 0015).
    pub follow: FollowConfig,
    /// The last-seen snapshot store, or `None` when it could not be opened.
    pub seen: Option<seen::Store>,
    /// Code highlighting for fences and source files (ADR 0016).
    pub highlighter: Arc<Highlighter>,
    /// Which files render as Markdown (ADR 0016).
    pub markdown: MarkdownConfig,
}

#[cfg(test)]
impl Options {
    /// Options for a test app rooted at `root`: no stores, plain code.
    pub(crate) fn for_test(root: PathBuf) -> Self {
        use fathomable_core::session::Id;
        Self {
            record: Record::new(Id::mint(), root, None),
            store: None,
            follow: FollowConfig::default(),
            seen: None,
            highlighter: Arc::new(Highlighter::plain()),
            markdown: MarkdownConfig::default(),
        }
    }
}

/// Run the app until the user quits, showing `open` first when given,
/// else the tree (ADR 0012).
pub fn run(
    workspace: Workspace,
    options: Options,
    theme: &Theme,
    open: Option<&Path>,
) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start async runtime")?;
    runtime.block_on(run_async(workspace, options, theme, open))
}

/// Restores the terminal on drop so a panic or error never leaves raw mode on.
struct TerminalGuard {
    /// Whether keyboard enhancement flags were pushed and must be popped.
    enhanced: bool,
}

impl TerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        let mut guard = Self { enhanced: false };
        guard.resume()?;
        Ok(guard)
    }

    /// Raw mode, the alternate screen, mouse capture, bracketed paste, and
    /// the kitty flags: on entry and again after `$EDITOR` gives the
    /// terminal back.
    fn resume(&mut self) -> anyhow::Result<()> {
        enable_raw_mode().context("cannot enable raw mode")?;
        crossterm::execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            SetCursorStyle::SteadyBlock
        )
        .context("cannot enter alternate screen")?;
        // Kitty-protocol disambiguation lets Ctrl-Enter differ from Enter in
        // the comment box (ADR 0013); terminals without it still get Alt-Enter.
        self.enhanced = matches!(
            crossterm::terminal::supports_keyboard_enhancement(),
            Ok(true)
        ) && crossterm::execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
        tracing::info!(enhanced = self.enhanced, "keyboard enhancement");
        Ok(())
    }

    /// Give the terminal back to the shell (or `$EDITOR`).
    fn leave(&mut self) {
        if self.enhanced {
            let _ = crossterm::execute!(io::stdout(), PopKeyboardEnhancementFlags);
            self.enhanced = false;
        }
        let _ = crossterm::execute!(
            io::stdout(),
            SetCursorStyle::DefaultUserShape,
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = disable_raw_mode();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.leave();
    }
}

/// The input thread's state: reading, asked to pause, or paused while
/// `$EDITOR` owns the terminal (ADR 0018).
const INPUT_READING: u8 = 0;
const INPUT_PAUSE_REQUESTED: u8 = 1;
const INPUT_PAUSED: u8 = 2;

/// The input thread: reads terminal events until it is told to pause.
struct Input {
    events: mpsc::Receiver<io::Result<Event>>,
    state: Arc<AtomicU8>,
}

impl Input {
    /// Stop the thread reading, and wait until it has, so nothing typed
    /// into the editor is swallowed here.
    async fn pause(&self) {
        self.state.store(INPUT_PAUSE_REQUESTED, Ordering::SeqCst);
        while self.state.load(Ordering::SeqCst) != INPUT_PAUSED {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    fn resume(&self) {
        self.state.store(INPUT_READING, Ordering::SeqCst);
    }
}

fn spawn_input() -> anyhow::Result<Input> {
    let (input_tx, events) = mpsc::channel::<io::Result<Event>>(64);
    let state = Arc::new(AtomicU8::new(INPUT_READING));
    let flag = Arc::clone(&state);
    thread::Builder::new()
        .name("input".to_owned())
        .spawn(move || {
            loop {
                match flag.load(Ordering::SeqCst) {
                    INPUT_PAUSE_REQUESTED => flag.store(INPUT_PAUSED, Ordering::SeqCst),
                    INPUT_PAUSED => {
                        thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    _ => {}
                }
                // Poll rather than block so a pause request is seen within
                // a beat; `poll` consumes nothing.
                match crossterm::event::poll(Duration::from_millis(100)) {
                    Ok(false) => continue,
                    Ok(true) => {}
                    Err(error) => {
                        let _ = input_tx.blocking_send(Err(error));
                        break;
                    }
                }
                let event = crossterm::event::read();
                let failed = event.is_err();
                if input_tx.blocking_send(event).is_err() || failed {
                    break;
                }
            }
        })
        .context("cannot start input thread")?;
    Ok(Input { events, state })
}

/// `Ctrl-e` in the comment box: hand the draft to `$VISUAL` or `$EDITOR`
/// on a temporary file and load the result back (ADR 0018). The comment
/// is not submitted; the socket and watcher wait while the editor runs.
async fn edit_draft(
    app: &mut App,
    guard: &mut TerminalGuard,
    input: &Input,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> anyhow::Result<()> {
    let Some(draft) = app.compose_draft() else {
        return Ok(());
    };
    let editor = ["VISUAL", "EDITOR"].iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    });
    let Some(editor) = editor else {
        app.notice("set $VISUAL or $EDITOR to edit the comment there");
        return Ok(());
    };
    let path = std::env::temp_dir().join(format!("fathomable-comment-{}.md", std::process::id()));
    fs::write(&path, draft).with_context(|| format!("cannot write {}", path.display()))?;
    input.pause().await;
    guard.leave();
    tracing::info!(%editor, path = %path.display(), "editing the comment draft");
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$1\""))
        .arg("fathomable")
        .arg(&path)
        .status();
    guard.resume()?;
    input.resume();
    terminal.clear().context("cannot redraw after the editor")?;
    match status {
        Ok(status) if status.success() => {
            let text = fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}", path.display()))?;
            app.set_compose_text(text.strip_suffix('\n').unwrap_or(&text));
            app.notice("draft loaded from the editor; Ctrl-Enter submits");
        }
        Ok(status) => app.notice(format!("{editor} exited with {status}; draft kept")),
        Err(error) => app.notice(format!("cannot run {editor}: {error}")),
    }
    let _ = fs::remove_file(&path);
    Ok(())
}

/// Watches the workspace root recursively (ADR 0015). When that fails
/// (inotify limits), falls back to the directory of the visible document,
/// following it as it changes (a rename lands as a directory event, so the
/// file itself is never watched directly).
struct DocWatcher {
    watcher: notify::RecommendedWatcher,
    target: Option<PathBuf>,
    recursive: bool,
}

impl DocWatcher {
    fn new() -> anyhow::Result<(Self, mpsc::UnboundedReceiver<PathBuf>)> {
        let (tx, rx) = mpsc::unbounded_channel::<PathBuf>();
        let watcher =
            notify::recommended_watcher(
                move |result: notify::Result<notify::Event>| match result {
                    // `notify` also reports opens (`Access`): our own
                    // reads of the document, `.gitignore`, and `.git`
                    // would otherwise feed back as changes, forever.
                    Ok(event) => {
                        tracing::debug!(kind = ?event.kind, paths = ?event.paths, "watcher event");
                        if is_change(event.kind) {
                            for path in event.paths {
                                let _ = tx.send(path);
                            }
                        }
                    }
                    Err(error) => tracing::warn!(%error, "file watcher error"),
                },
            )
            .context("cannot create file watcher")?;
        Ok((
            Self {
                watcher,
                target: None,
                recursive: false,
            },
            rx,
        ))
    }

    /// Watch everything under `root`; false when the watch cannot be set up.
    fn watch_root(&mut self, root: &Path) -> bool {
        match self.watcher.watch(root, RecursiveMode::Recursive) {
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

    fn follow(&mut self, target: Option<&Path>) {
        if self.recursive || target == self.target.as_deref() {
            return;
        }
        if let Some(old) = self.target.as_ref().and_then(|p| p.parent()) {
            let _ = self.watcher.unwatch(old);
        }
        if let Some(dir) = target.and_then(Path::parent)
            && let Err(error) = self.watcher.watch(dir, RecursiveMode::NonRecursive)
        {
            tracing::warn!(%error, dir = %dir.display(), "cannot watch directory");
        }
        self.target = target.map(Path::to_path_buf);
    }

    fn is_target(&self, path: &Path) -> bool {
        self.recursive || self.target.as_deref() == Some(path)
    }
}

/// Whether a path under `.git` can move HEAD or the index. Object
/// writes, reflogs, and lock files churn constantly and change neither.
fn is_git_metadata(relative: &Path) -> bool {
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

/// Events that mean a file's content or existence may have changed.
fn is_change(kind: notify::EventKind) -> bool {
    use notify::EventKind;
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    )
}

/// Watcher paths waiting out the hint debounce (ADR 0015).
#[derive(Default)]
struct ChangeBatch {
    paths: Vec<PathBuf>,
    flush_at: Option<Instant>,
}

impl ChangeBatch {
    /// Adds a path; the first one after a flush starts the quiet period.
    fn push(&mut self, path: PathBuf, debounce: Duration) {
        self.paths.push(path);
        self.flush_at
            .get_or_insert_with(|| Instant::now() + debounce);
    }

    /// Resolves once the quiet period ends; never while the batch is empty.
    async fn settled(&self) {
        match self.flush_at {
            Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
            None => std::future::pending().await,
        }
    }

    fn take(&mut self) -> Vec<PathBuf> {
        self.flush_at = None;
        std::mem::take(&mut self.paths)
    }
}

fn serve_socket(record: &Record, app: mpsc::Sender<socket::Envelope>) -> Option<socket::Serving> {
    let Some(path) = record.socket() else {
        tracing::warn!("XDG_RUNTIME_DIR unset; no session socket");
        return None;
    };
    match socket::Listener::bind(path) {
        Ok(listener) => Some(listener.serve(record.clone(), app)),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "cannot listen on session socket");
            None
        }
    }
}

async fn run_async(
    workspace: Workspace,
    options: Options,
    theme: &Theme,
    open: Option<&Path>,
) -> anyhow::Result<()> {
    let (mut doc_watcher, mut reload_rx) = DocWatcher::new()?;
    let (request_tx, mut request_rx) = mpsc::channel::<socket::Envelope>(16);
    let _socket = serve_socket(&options.record, request_tx);
    let mut sigterm = signal(SignalKind::terminate()).context("cannot listen for SIGTERM")?;
    let mut sighup = signal(SignalKind::hangup()).context("cannot listen for SIGHUP")?;

    // The guard queries the terminal, so it must run before the input
    // thread starts consuming responses.
    let mut guard = TerminalGuard::enter()?;
    let mut input = spawn_input()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).context("cannot initialise terminal")?;
    let theme = ui::Theme::from_core(theme);
    let size = terminal.size().context("cannot read terminal size")?;
    let hint_debounce = options.follow.hint_debounce;
    let watching = doc_watcher.watch_root(workspace.root());
    let mut app = App::new(
        workspace,
        usize::from(size.width),
        usize::from(size.height),
        options,
    );
    app.set_watching_root(watching);
    match open {
        Some(path) => app.open(path),
        None => app.show_sidebar(),
    }

    let mut batch = ChangeBatch::default();
    loop {
        doc_watcher.follow(app.current_abs_path());
        app.settle();
        terminal
            .draw(|frame| ui::draw(frame, &app, &theme))
            .context("draw failed")?;
        let effect = tokio::select! {
            event = input.events.recv() => match event {
                Some(Ok(event)) => {
                    let mut effect = handle_event(&mut app, &event);
                    // Coalesce a burst (wheel flick, key repeat) into one
                    // frame: draining here keeps the redraw from lagging
                    // behind the queue and jumping several notches at once.
                    while matches!(effect, Effect::None)
                        && let Ok(next) = input.events.try_recv()
                    {
                        let next = next.context("reading terminal input")?;
                        effect = handle_event(&mut app, &next);
                    }
                    effect
                }
                Some(Err(error)) => return Err(error).context("reading terminal input"),
                None => Effect::Quit,
            },
            notice = reload_rx.recv() => {
                if let Some(path) = notice {
                    // One quiet period turns a burst of writes into one
                    // change per file (ADR 0015 `follow.hint-debounce`).
                    // The wait is its own arm below, so input keeps
                    // flowing while the burst settles.
                    batch.push(path, hint_debounce);
                    while let Ok(path) = reload_rx.try_recv() {
                        batch.push(path, hint_debounce);
                    }
                }
                Effect::None
            }
            () = batch.settled() => {
                let mut changed = batch.take();
                changed.retain(|p| doc_watcher.is_target(p));
                app.on_changes(changed);
                Effect::None
            }
            () = tokio::time::sleep(app.tick_in().unwrap_or(Duration::from_hours(1))) => {
                app.tick();
                Effect::None
            }
            envelope = request_rx.recv() => {
                if let Some(socket::Envelope { request, reply }) = envelope {
                    let response = app.handle_request(request);
                    let _ = reply.send(response);
                }
                Effect::None
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM; quitting");
                Effect::Quit
            }
            _ = sighup.recv() => {
                tracing::info!("SIGHUP; quitting");
                Effect::Quit
            }
        };
        match effect {
            Effect::None => {}
            Effect::Quit => break,
            Effect::Copy(text) => {
                tracing::debug!(bytes = text.len(), "copied selection via OSC 52");
                clipboard::copy(&text).context("cannot write to clipboard")?;
            }
            Effect::Command(command) => app.command(&command),
            Effect::EditDraft => edit_draft(&mut app, &mut guard, &input, &mut terminal).await?,
        }
    }
    app.on_quit();
    tracing::info!("app closed");
    Ok(())
}

fn handle_event(app: &mut App, event: &Event) -> Effect {
    match event {
        Event::Key(key) if key.kind != crossterm::event::KeyEventKind::Release => {
            keys::handle_key(app, *key)
        }
        Event::Mouse(mouse) => keys::handle_mouse(app, *mouse),
        Event::Paste(text) => {
            app.paste(text);
            Effect::None
        }
        Event::Resize(width, height) => {
            app.resize(usize::from(*width), usize::from(*height));
            Effect::None
        }
        _ => Effect::None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use std::sync::Arc;

    use fathomable_core::config::{FollowConfig, MarkdownConfig};
    use fathomable_core::highlight::Highlighter;
    use fathomable_core::workspace::{Workspace, WorkspaceError, open_options};

    use super::{App, Focus, Options, PickerKind, Popup, is_change, is_git_metadata};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> std::io::Result<Self> {
            let dir =
                std::env::temp_dir().join(format!("fathomable-app-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("docs"))?;
            fs::write(dir.join("README.md"), "# Readme\n\nhello\n")?;
            fs::write(dir.join("docs/guide.md"), "# Guide\n")?;
            fs::write(dir.join("docs/notes.md"), "# Notes\n")?;
            Ok(Self(dir))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
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
        let dir = TempDir::new("syntax")?;
        fs::write(dir.0.join("main.rs"), "fn main() {}\n")?;
        fs::write(dir.0.join("LICENSE"), "# Terms\n")?;
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
            "extensionless files render as Markdown"
        );

        // A narrower list flips both.
        let mut app = app_with(
            &dir,
            Options {
                markdown: MarkdownConfig {
                    extensions: vec!["rs".to_owned()],
                    extensionless: false,
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
    fn opening_files_builds_history_and_recent_list() -> anyhow::Result<()> {
        let dir = TempDir::new("history")?;
        let mut app = app(&dir)?;
        assert_eq!(app.current_path(), Path::new(""));
        app.open(Path::new("README.md"));
        app.open(Path::new("docs/guide.md"));
        assert_eq!(app.current_path(), Path::new("docs/guide.md"));
        app.history_back();
        assert_eq!(app.current_path(), Path::new("README.md"));
        app.history_back();
        assert_eq!(app.message(), Some("at oldest file"));
        app.history_forward();
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

        let dir = TempDir::new("narrow")?;
        gix::ThreadSafeRepository::init_opts(
            &dir.0,
            gix::create::Kind::WithWorktree,
            gix::create::Options::default(),
            open_options(),
        )?;
        commit_and_stage(
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
        let theme = crate::app::ui::Theme::from_core(&core);
        for width in 1..=40u16 {
            app.resize(usize::from(width), 12);
            let mut terminal = Terminal::new(TestBackend::new(width, 12))?;
            terminal.draw(|frame| crate::app::ui::draw(frame, &app, &theme))?;
            let buffer = terminal.backend().buffer().clone();
            let divider = u16::try_from(app.sidebar_width())?.saturating_sub(1);
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
        let dir = TempDir::new("sidebar")?;
        let mut app = app(&dir)?;
        assert_eq!(app.sidebar_width(), 0);
        app.open(Path::new("docs/notes.md"));
        app.toggle_sidebar_focus();
        assert_eq!(app.focus(), Focus::Sidebar);
        assert_eq!(app.sidebar_width(), 32);
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
    fn left_at_column_zero_hands_focus_to_the_tree() -> anyhow::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use super::keys;
        let dir = TempDir::new("left")?;
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
        let dir = TempDir::new("picker")?;
        let mut app = app(&dir)?;
        app.open_space_menu();
        app.space_menu_select('f');
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
    fn watcher_keeps_writes_and_drops_reads() {
        use notify::EventKind;
        use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind};
        assert!(is_change(EventKind::Create(CreateKind::File)));
        assert!(is_change(EventKind::Modify(ModifyKind::Any)));
        assert!(is_change(EventKind::Remove(RemoveKind::File)));
        assert!(!is_change(EventKind::Access(AccessKind::Open(
            notify::event::AccessMode::Read
        ))));
        assert!(!is_change(EventKind::Any));
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
    fn unchanged_content_queues_nothing() -> anyhow::Result<()> {
        let dir = TempDir::new("touch")?;
        let mut app = app(&dir)?;
        app.open(Path::new("README.md"));
        app.open(Path::new("docs/guide.md"));
        // A touch (or our own read, reported as a change) on an open file
        // whose text is identical must not hint.
        app.on_changes(vec![dir.0.join("README.md"), dir.0.join("README.md")]);
        assert!(app.queue().is_empty());
        Ok(())
    }

    fn changed(app: &mut App, dir: &TempDir, relative: &str, text: &str) -> std::io::Result<()> {
        let absolute = dir.0.join(relative);
        fs::write(&absolute, text)?;
        app.on_changes(vec![absolute]);
        Ok(())
    }

    /// Write `files` (root-relative, content) as a tree object, nested
    /// directories and all, and return its id.
    fn write_tree(repo: &gix::Repository, files: &[(&str, &str)]) -> anyhow::Result<gix::ObjectId> {
        use std::collections::BTreeMap;
        let mut entries = Vec::new();
        let mut subdirs: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
        for (path, content) in files {
            match path.split_once('/') {
                Some((dir, rest)) => subdirs.entry(dir).or_default().push((rest, content)),
                None => entries.push(gix::objs::tree::Entry {
                    mode: gix::objs::tree::EntryKind::Blob.into(),
                    filename: (*path).into(),
                    oid: repo.write_blob(content.as_bytes())?.detach(),
                }),
            }
        }
        for (dir, files) in subdirs {
            entries.push(gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Tree.into(),
                filename: dir.into(),
                oid: write_tree(repo, &files)?,
            });
        }
        entries.sort();
        Ok(repo.write_object(gix::objs::Tree { entries })?.detach())
    }

    /// Commit `files` as `HEAD` and stage the same tree, as `git add -A`
    /// then `git commit` would leave things.
    fn commit_and_stage(root: &Path, files: &[(&str, &str)]) -> anyhow::Result<()> {
        let repo = gix::open_opts(root, open_options())?;
        let tree = write_tree(&repo, files)?;
        let signature = gix::actor::SignatureRef {
            name: "test".into(),
            email: "test@example.com".into(),
            time: "0 +0000",
        };
        let parent = repo.head_id().ok().map(gix::Id::detach);
        repo.commit_as(signature, signature, "HEAD", "commit", tree, parent)?;
        stage(root, files)
    }

    /// Replace the index with `files`.
    fn stage(root: &Path, files: &[(&str, &str)]) -> anyhow::Result<()> {
        let repo = gix::open_opts(root, open_options())?;
        let tree = write_tree(&repo, files)?;
        let state = gix::index::State::from_tree(
            &tree,
            &repo.objects,
            gix::validate::path::component::Options::default(),
        )?;
        let mut file = gix::index::File::from_state(state, repo.index_path());
        file.write(gix::index::write::Options::default())?;
        Ok(())
    }

    #[test]
    #[expect(clippy::too_many_lines, reason = "one walk through the whole key set")]
    fn hunks_cross_uncommitted_files_in_path_order() -> anyhow::Result<()> {
        use fathomable_core::status::State;

        let dir = TempDir::new("hunks")?;
        gix::ThreadSafeRepository::init_opts(
            &dir.0,
            gix::create::Kind::WithWorktree,
            gix::create::Options::default(),
            open_options(),
        )?;
        let committed = [
            ("README.md", "# Readme\n\nhello\n"),
            ("docs/guide.md", "# Guide\n"),
            ("docs/notes.md", "# Notes\n"),
        ];
        commit_and_stage(&dir.0, &committed)?;
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
        stage(
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
        commit_and_stage(
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
        let dir = TempDir::new("changes")?;
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
    fn new_and_removed_files_update_the_tree() -> anyhow::Result<()> {
        let dir = TempDir::new("tree-watch")?;
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
        let dir = TempDir::new("source")?;
        let mut app = app(&dir)?;
        app.open(Path::new("README.md"));
        changed(&mut app, &dir, "README.md", "# Readme\n\nchanged\n")?;
        assert_eq!(app.queue().len(), 1, "an edit to the open file hints");
        assert!(
            app.view().text().contains("changed"),
            "and the open file reloads"
        );

        let follow = FollowConfig {
            ignore: vec!["docs/**".to_owned()],
            toast: std::time::Duration::ZERO,
            ..FollowConfig::default()
        };
        let mut app = app_with(
            &dir,
            Options {
                follow,
                ..Options::for_test(dir.0.clone())
            },
        )?;
        changed(&mut app, &dir, "docs/guide.md", "# Guide\n\n3\n")?;
        assert!(app.queue().is_empty(), "follow.ignore globs apply");
        changed(&mut app, &dir, "README.md", "# Readme\n\nx\n")?;
        assert_eq!(app.queue().len(), 1);
        assert!(app.toasts().is_empty(), "toast 0 disables toasts");

        app.command("follow");
        assert!(app.auto_jump());
        app.command("follow off");
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
        let dir = TempDir::new("seen")?;
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
        let idle = FollowConfig {
            seen_idle: std::time::Duration::from_millis(1),
            ..FollowConfig::default()
        };
        let mut app2 = app_with(
            &dir,
            Options {
                follow: idle,
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
        let dir = TempDir::new("auto")?;
        let follow = FollowConfig {
            auto: true,
            jump_debounce: std::time::Duration::ZERO,
            ..FollowConfig::default()
        };
        let mut app = app_with(
            &dir,
            Options {
                follow,
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
        let follow = FollowConfig {
            auto: true,
            jump_debounce: std::time::Duration::ZERO,
            ..FollowConfig::default()
        };
        let mut app = app_with(
            &dir,
            Options {
                follow,
                ..Options::for_test(dir.0.clone())
            },
        )?;
        changed(&mut app, &dir, "docs/notes.md", "# Notes\n\nnewer\n")?;
        app.tick();
        assert_eq!(app.current_path(), Path::new("docs/notes.md"));
        assert!(app.queue().is_empty());

        app.open_space_menu();
        app.space_menu_select('j');
        assert!(matches!(app.popup(), Some(Popup::Jump)));
        app.jump_menu_select('a');
        assert!(!app.auto_jump());
        assert!(app.popup().is_none());
        Ok(())
    }

    #[test]
    fn agent_open_queues_a_settled_range_and_session_info_reports_state() -> anyhow::Result<()> {
        use fathomable_core::session::{Request, Response};

        let dir = TempDir::new("agent")?;
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
}
