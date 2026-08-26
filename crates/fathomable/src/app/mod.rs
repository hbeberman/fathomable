// @okf-doc: /decisions/0012-workspace-mode.md
//! The workspace app: open documents, sidebar, popups, event loop, socket.
//!
//! [`App`] is plain state so ADR 0012 behaviour can be tested without a
//! terminal; `ui` draws it, `keys` drives it, and [`run`] owns the terminal,
//! the file watcher, and the session socket.

mod clipboard;
mod keys;
mod socket;
mod threads;
mod ui;
mod view;

use std::io;
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::Context;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fathomable_core::Document;
use fathomable_core::annotations::{Store, ThreadId};
use fathomable_core::picker::{Match, Picker};
use fathomable_core::session::{Record, Request, Response};
use fathomable_core::theme::Theme;
use fathomable_core::tree::{Activation, Tree};
use fathomable_core::workspace::{Filter, Workspace};
use notify::{RecursiveMode, Watcher};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;

pub use threads::{Compose, Mark, ThreadPanel};
use view::{Effect, View};

/// How long to wait after a change notification before re-reading, so an
/// editor's write-then-rename lands as one reload.
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(40);

/// Sidebar width in columns before clamping to a third of the terminal.
const SIDEBAR_WIDTH: usize = 32;

/// Rows kept visible above and below the sidebar cursor.
const SIDEBAR_SCROLLOFF: usize = 2;

/// What the view shows before any file is open.
const WELCOME: &str = "# Fathomable\n\nNo file is open.\n\n\
- `Space f` opens the file picker\n\
- `Space e` opens the tree\n\
- `V` or a mouse drag selects lines; `y` copies, `c` comments\n\
- `Space ?` lists every key\n\
- `:q` quits\n";

/// Which pane receives keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    View,
    Sidebar,
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
    /// A thread being read.
    Thread(ThreadPanel),
}

/// One space-menu entry: key, label.
pub const SPACE_MENU: [(char, &str); 8] = [
    ('e', "toggle tree focus"),
    ('E', "hide tree"),
    ('f', "open file"),
    ('F', "open file (incl. ignored)"),
    ('o', "recent files"),
    ('a', "thread at cursor"),
    ('A', "threads in file"),
    ('?', "all keys"),
];

/// Every binding, for `Space ?`.
pub const HELP: [(&str, &str); 30] = [
    ("j / k", "move down / up"),
    ("h / l", "move left / right"),
    ("gg / G", "top / bottom"),
    ("Ctrl-d / Ctrl-u", "half page down / up"),
    ("/ ?", "search forward / backward"),
    ("n / N", "next / previous match"),
    ("V / mouse drag", "select lines / cells"),
    ("y (selected)", "copy source to clipboard"),
    ("c (selected)", "comment on the selection"),
    ("Space a", "read thread at cursor"),
    ("Space A", "pick a thread in this file"),
    ("]a / [a", "next / previous thread"),
    ("thread r x n p j k", "reply, resolve, switch, scroll"),
    ("comment Enter", "newline; Ctrl-Enter or Alt-Enter submits"),
    ("gs", "toggle source view"),
    ("gd / :diff", "toggle diff view against HEAD"),
    ("]c / [c", "next / previous change"),
    ("[o / ]o", "previous / next opened file"),
    (":N", "go to source line N"),
    (":noh", "clear search highlight"),
    (":q", "quit"),
    ("Space e / Ctrl-b", "tree: open and focus, or return focus"),
    ("Space E", "tree: hide"),
    ("Space f / F", "file picker / including ignored"),
    ("Space o", "recent files"),
    ("tree j k h l Enter", "move, collapse, expand or open"),
    ("tree R", "re-read directories"),
    ("tree I", "show ignored entries"),
    ("picker Up Down Ctrl-j Ctrl-k", "move selection"),
    ("Esc", "close / clear"),
];

#[derive(Debug)]
struct Doc {
    document: Document,
    relative: PathBuf,
    view: View,
    marks: Vec<Mark>,
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
}

impl App {
    /// Start with no document open, for a terminal of `width` by `height`;
    /// `store` is `None` when the thread file could not be opened.
    pub fn new(
        workspace: Workspace,
        width: usize,
        height: usize,
        session: String,
        store: Option<Store>,
    ) -> Self {
        let mut app = Self {
            workspace,
            docs: Vec::new(),
            current: None,
            history: Vec::new(),
            history_pos: 0,
            welcome: View::new(WELCOME.to_owned(), 1, 1),
            tree: None,
            sidebar_visible: false,
            sidebar_scroll: 0,
            focus: Focus::View,
            popup: None,
            file_index: None,
            all_index: None,
            message: None,
            pending: None,
            width,
            height,
            session,
            store,
            followed: Vec::new(),
        };
        app.relayout();
        app
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

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
        if self.sidebar_visible {
            SIDEBAR_WIDTH.min(self.width / 3)
        } else {
            0
        }
    }

    /// Rows available to panes once the status line is taken.
    pub fn pane_rows(&self) -> usize {
        self.height.saturating_sub(1).max(1)
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.relayout();
    }

    fn relayout(&mut self) {
        let rows = self.pane_rows();
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
                    let view = View::new(document.text().to_owned(), 1, 1);
                    self.docs.push(Doc {
                        document,
                        relative: relative.clone(),
                        view,
                        marks: Vec::new(),
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
                Response::Error("session_info is answered by the socket".to_owned())
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

    /// Re-read a document whose file changed on disk; `absolute` is the
    /// watcher's path.
    pub fn reload(&mut self, absolute: &Path) {
        let Some(index) = self
            .docs
            .iter()
            .position(|doc| doc.document.path() == absolute)
        else {
            return;
        };
        let doc = &mut self.docs[index];
        match doc.document.reload() {
            Ok(true) => {
                tracing::info!(path = %doc.relative.display(), "reloaded after change");
                doc.view.reload(doc.document.text().to_owned());
                self.refresh_base(index);
                self.refresh_marks(index);
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "reload failed; keeping previous text"),
        }
    }

    /// Re-read the `HEAD` text of the document at `index` as its diff base
    /// (ADR 0006). A read failure is reported once and leaves no base.
    fn refresh_base(&mut self, index: usize) {
        let Some(doc) = self.docs.get(index) else {
            return;
        };
        let base = match self.workspace.head_text(&doc.relative) {
            Ok(base) => base,
            Err(error) => {
                self.notice(format!("no diff base: {error}"));
                None
            }
        };
        if let Some(doc) = self.docs.get_mut(index) {
            doc.view.set_base(base);
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

    /// `Space e` / `Ctrl-b`: open and focus the tree, or hand focus back.
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
            '?' => self.open_help(),
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

/// Everything [`run`] needs beyond the workspace.
#[derive(Debug)]
pub struct Options<'a> {
    /// Root-relative file to open first, if any.
    pub open: Option<PathBuf>,
    pub record: &'a Record,
    pub theme: &'a Theme,
    /// The workspace's thread store, or `None` when it could not be opened.
    pub store: Option<Store>,
}

/// Run the app until the user quits.
pub fn run(workspace: Workspace, options: Options<'_>) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start async runtime")?;
    runtime.block_on(run_async(workspace, options))
}

/// Restores the terminal on drop so a panic or error never leaves raw mode on.
struct TerminalGuard {
    /// Whether keyboard enhancement flags were pushed and must be popped.
    enhanced: bool,
}

impl TerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode().context("cannot enable raw mode")?;
        crossterm::execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)
            .context("cannot enter alternate screen")?;
        // Kitty-protocol disambiguation lets Ctrl-Enter differ from Enter in
        // the comment box (ADR 0013); terminals without it still get Alt-Enter.
        let enhanced = matches!(
            crossterm::terminal::supports_keyboard_enhancement(),
            Ok(true)
        ) && crossterm::execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
        tracing::info!(enhanced, "keyboard enhancement");
        Ok(Self { enhanced })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.enhanced {
            let _ = crossterm::execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = crossterm::execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

fn spawn_input() -> anyhow::Result<mpsc::Receiver<io::Result<Event>>> {
    let (input_tx, input_rx) = mpsc::channel::<io::Result<Event>>(64);
    thread::Builder::new()
        .name("input".to_owned())
        .spawn(move || {
            loop {
                let event = crossterm::event::read();
                let failed = event.is_err();
                if input_tx.blocking_send(event).is_err() || failed {
                    break;
                }
            }
        })
        .context("cannot start input thread")?;
    Ok(input_rx)
}

/// Watches the directory of the visible document, following it as it
/// changes (a rename lands as a directory event, so the file itself is
/// never watched directly).
struct DocWatcher {
    watcher: notify::RecommendedWatcher,
    target: Option<PathBuf>,
}

impl DocWatcher {
    fn new() -> anyhow::Result<(Self, mpsc::UnboundedReceiver<PathBuf>)> {
        let (tx, rx) = mpsc::unbounded_channel::<PathBuf>();
        let watcher =
            notify::recommended_watcher(
                move |result: notify::Result<notify::Event>| match result {
                    Ok(event) => {
                        for path in event.paths {
                            let _ = tx.send(path);
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
            },
            rx,
        ))
    }

    fn follow(&mut self, target: Option<&Path>) {
        if target == self.target.as_deref() {
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
        self.target.as_deref() == Some(path)
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

async fn run_async(workspace: Workspace, options: Options<'_>) -> anyhow::Result<()> {
    let (mut doc_watcher, mut reload_rx) = DocWatcher::new()?;
    let (request_tx, mut request_rx) = mpsc::channel::<socket::Envelope>(16);
    let _socket = serve_socket(options.record, request_tx);
    let mut sigterm = signal(SignalKind::terminate()).context("cannot listen for SIGTERM")?;
    let mut sighup = signal(SignalKind::hangup()).context("cannot listen for SIGHUP")?;

    // The guard queries the terminal, so it must run before the input
    // thread starts consuming responses.
    let _guard = TerminalGuard::enter()?;
    let mut input_rx = spawn_input()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).context("cannot initialise terminal")?;
    let theme = ui::Theme::from_core(options.theme);
    let size = terminal.size().context("cannot read terminal size")?;
    let mut app = App::new(
        workspace,
        usize::from(size.width),
        usize::from(size.height),
        options.record.id().to_string(),
        options.store,
    );
    if let Some(path) = &options.open {
        app.open(path);
    }

    loop {
        doc_watcher.follow(app.current_abs_path());
        terminal
            .draw(|frame| ui::draw(frame, &app, &theme))
            .context("draw failed")?;
        let effect = tokio::select! {
            event = input_rx.recv() => match event {
                Some(Ok(event)) => {
                    let mut effect = handle_event(&mut app, &event);
                    // Coalesce a burst (wheel flick, key repeat) into one
                    // frame: draining here keeps the redraw from lagging
                    // behind the queue and jumping several notches at once.
                    while matches!(effect, Effect::None)
                        && let Ok(next) = input_rx.try_recv()
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
                if let Some(first) = notice {
                    tokio::time::sleep(RELOAD_DEBOUNCE).await;
                    let mut changed = vec![first];
                    while let Ok(path) = reload_rx.try_recv() {
                        changed.push(path);
                    }
                    if let Some(target) = changed.into_iter().find(|p| doc_watcher.is_target(p)) {
                        app.reload(&target);
                    }
                }
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
        }
    }
    tracing::info!("app closed");
    Ok(())
}

fn handle_event(app: &mut App, event: &Event) -> Effect {
    match event {
        Event::Key(key) if key.kind != crossterm::event::KeyEventKind::Release => {
            keys::handle_key(app, *key)
        }
        Event::Mouse(mouse) => keys::handle_mouse(app, *mouse),
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

    use fathomable_core::workspace::{Workspace, WorkspaceError};

    use super::{App, Focus, PickerKind, Popup};

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
        let workspace = Workspace::discover(&dir.0)?;
        Ok(App::new(workspace, 100, 30, "test".to_owned(), None))
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
}
