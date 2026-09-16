// @okf-doc: /decisions/0081-the-menu-bar.md
//! The persistent menu bar: its stable workflow menus, passive context,
//! one-level fly-outs, geometry, and mouse/keyboard navigation.

use fathomable_core::layout::display_width;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::input::bindings::{self, Action, Chord, Key, Where};
use crate::app::view::Effect;
use crate::app::{App, Focus};

const FULL_LABEL_WIDTH: usize = 21;
const MENU_TOP: usize = 1;
const STATUS_ROWS: usize = 1;
const BORDER_ROWS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Root {
    App,
    Go,
    Review,
    Diff,
}

impl Root {
    pub(crate) const ALL: [Self; 4] = [Self::App, Self::Go, Self::Review, Self::Diff];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::App => "☰",
            Self::Go => "Go",
            Self::Review => "Review",
            Self::Diff => "Diff",
        }
    }

    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::App => "Fathomable",
            Self::Go => "Go",
            Self::Review => "Review",
            Self::Diff => "Diff",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Submenu {
    Layout,
    Help,
    Go,
    Review,
    Diff,
}

impl Submenu {
    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::Layout => "Layout",
            Self::Help => "Help",
            Self::Go => "Go",
            Self::Review => "Review",
            Self::Diff => "Diff",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    Action(Action),
    ReviewToggle,
    ReviewResolved,
    ReviewFile,
    GettingStarted,
    Doctor,
    Status,
    About,
    Quit,
    Submenu(Submenu),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Item {
    pub(crate) label: String,
    pub(crate) hint: String,
    pub(crate) enabled: bool,
    pub(crate) checked: bool,
    pub(crate) target: Target,
}

impl Item {
    fn action(app: &App, action: Action, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            hint: bindings::hint(Where::View, action).unwrap_or_default(),
            enabled: action_available(app, action),
            checked: action_checked(app, action),
            target: Target::Action(action),
        }
    }

    fn command(label: &'static str, hint: &'static str, target: Target) -> Self {
        Self {
            label: label.to_owned(),
            hint: hint.to_owned(),
            enabled: true,
            checked: false,
            target,
        }
    }

    fn submenu(label: &'static str, submenu: Submenu) -> Self {
        Self::command(label, "", Target::Submenu(submenu))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Row {
    Item(Item),
    Separator,
}

impl Row {
    pub(crate) fn item(&self) -> Option<&Item> {
        match self {
            Self::Item(item) => Some(item),
            Self::Separator => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focused {
    Label,
    Root(usize),
    Child(usize),
}

#[derive(Debug, Clone)]
pub(crate) struct Open {
    pub(crate) root: Root,
    pub(crate) focused: Focused,
    pub(crate) submenu: Option<(Submenu, usize)>,
    pub(crate) root_scroll: usize,
    pub(crate) child_scroll: usize,
    typed: Vec<Chord>,
}

#[derive(Debug)]
pub(crate) struct MenuBar {
    shown: bool,
    open: Option<Open>,
}

impl MenuBar {
    pub(crate) const fn new(shown: bool) -> Self {
        Self { shown, open: None }
    }

    pub(crate) const fn shown(&self) -> bool {
        self.shown
    }

    pub(crate) fn toggle(&mut self) {
        self.shown = !self.shown;
        if !self.shown {
            self.open = None;
        }
    }

    pub(crate) const fn open(&self) -> Option<&Open> {
        self.open.as_ref()
    }

    pub(crate) fn open_mut(&mut self) -> Option<&mut Open> {
        self.open.as_mut()
    }

    pub(crate) fn show(&mut self, root: Root) {
        self.open = Some(Open {
            root,
            focused: Focused::Label,
            submenu: None,
            root_scroll: 0,
            child_scroll: 0,
            typed: Vec::new(),
        });
    }

    pub(crate) fn close(&mut self) {
        self.open = None;
    }
}

fn next_item(rows: &[Row], from: Option<usize>, delta: isize) -> Option<usize> {
    let enabled = |index: usize| {
        rows.get(index)
            .and_then(Row::item)
            .is_some_and(|item| item.enabled)
    };
    if delta >= 0 {
        let start = from.map_or(0, |index| index.saturating_add(1));
        (start..rows.len()).find(|index| enabled(*index))
    } else {
        let end = from.unwrap_or(rows.len());
        (0..end).rev().find(|index| enabled(*index))
    }
}

fn keep_visible(scroll: &mut usize, selected: usize, visible: usize, total: usize) {
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= scroll.saturating_add(visible) {
        *scroll = selected.saturating_sub(visible.saturating_sub(1));
    }
    *scroll = (*scroll).min(total.saturating_sub(visible));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Label {
    pub(crate) root: Root,
    pub(crate) x: usize,
    pub(crate) width: usize,
}

pub(crate) fn labels(width: usize) -> Vec<Label> {
    let roots: &[Root] = if width < FULL_LABEL_WIDTH {
        &Root::ALL[..1]
    } else {
        &Root::ALL
    };
    let mut x = 0;
    roots
        .iter()
        .map(|root| {
            let width = display_width(root.label()) + 2;
            let label = Label {
                root: *root,
                x,
                width,
            };
            x += width;
            label
        })
        .collect()
}

pub(crate) fn label_at(width: usize, column: usize) -> Option<Root> {
    labels(width)
        .into_iter()
        .find(|label| column >= label.x && column < label.x + label.width)
        .map(|label| label.root)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MenuLayout {
    pub(crate) x: usize,
    pub(crate) y: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) scroll: usize,
    pub(crate) visible_rows: usize,
}

impl MenuLayout {
    pub(crate) fn row_at(self, column: usize, row: usize, rows: &[Row]) -> Option<usize> {
        if column <= self.x
            || column + 1 >= self.x + self.width
            || row <= self.y
            || row + 1 >= self.y + self.height
        {
            return None;
        }
        let index = self.scroll + row - self.y - 1;
        rows.get(index).and_then(Row::item).map(|_| index)
    }

    pub(crate) fn contains(self, column: usize, row: usize) -> bool {
        column >= self.x
            && column < self.x + self.width
            && row >= self.y
            && row < self.y + self.height
    }
}

pub(crate) fn root_layout(app: &App, rows: &[Row]) -> Option<MenuLayout> {
    let open = app.menu_bar.open()?;
    let label = labels(app.width)
        .into_iter()
        .find(|label| label.root == open.root)?;
    Some(layout_at(
        rows,
        open.root.title(),
        label.x,
        MENU_TOP,
        open.root_scroll,
        app.width,
        app.height,
    ))
}

pub(crate) fn child_layout(
    app: &App,
    root: MenuLayout,
    _root_rows: &[Row],
    child_rows: &[Row],
) -> Option<MenuLayout> {
    let open = app.menu_bar.open()?;
    let (submenu, parent) = open.submenu?;
    if parent < root.scroll || parent >= root.scroll + root.visible_rows {
        return None;
    }
    let y = root.y + 1 + parent - root.scroll;
    let wanted_x = root.x + root.width;
    let width = menu_width(child_rows, submenu.title()).min(app.width);
    let x = if wanted_x + width <= app.width {
        wanted_x
    } else {
        root.x.saturating_sub(width)
    };
    Some(layout_at(
        child_rows,
        submenu.title(),
        x,
        y,
        open.child_scroll,
        app.width,
        app.height,
    ))
}

fn layout_at(
    rows: &[Row],
    title: &str,
    x: usize,
    y: usize,
    scroll: usize,
    screen_width: usize,
    screen_height: usize,
) -> MenuLayout {
    let width = menu_width(rows, title).min(screen_width);
    let x = x.min(screen_width.saturating_sub(width));
    let available = screen_height.saturating_sub(STATUS_ROWS).saturating_sub(y);
    let visible_rows = rows.len().min(available.saturating_sub(BORDER_ROWS));
    let max_scroll = rows.len().saturating_sub(visible_rows);
    let scroll = scroll.min(max_scroll);
    MenuLayout {
        x,
        y,
        width,
        height: (visible_rows + BORDER_ROWS).min(available),
        scroll,
        visible_rows,
    }
}

fn menu_width(rows: &[Row], title: &str) -> usize {
    rows.iter()
        .filter_map(Row::item)
        .map(|item| {
            2 + 2
                + display_width(&item.label)
                + 1
                + display_width(&item.hint)
                + usize::from(matches!(item.target, Target::Submenu(_)))
        })
        .chain(std::iter::once(display_width(title) + 4))
        .max()
        .unwrap_or(8)
        .max(8)
}

#[expect(
    clippy::too_many_lines,
    reason = "one declarative match keeps the stable menu contents together"
)]
pub(crate) fn rows(app: &App, root: Root) -> Vec<Row> {
    match root {
        Root::App => {
            let mut rows = vec![
                Row::Item(Item::submenu("Layout", Submenu::Layout)),
                Row::Item(Item::submenu("Help", Submenu::Help)),
            ];
            if labels(app.width).len() == 1 {
                rows.extend([
                    Row::Item(Item::submenu("Go", Submenu::Go)),
                    Row::Item(Item::submenu("Review", Submenu::Review)),
                    Row::Item(Item::submenu("Diff", Submenu::Diff)),
                ]);
            }
            rows.extend([
                Row::Separator,
                Row::Item(Item::command("Status", ":status", Target::Status)),
                Row::Item(Item::command("About", ":about", Target::About)),
                Row::Item(Item::command("Quit", ":q", Target::Quit)),
            ]);
            rows
        }
        Root::Go => vec![
            Row::Item(Item::action(app, Action::PickFile, "Open file…")),
            Row::Item(Item::action(
                app,
                Action::PickAnyFile,
                "Open incl. ignored…",
            )),
            Row::Item(Item::action(app, Action::PickRecent, "Recent files…")),
            Row::Separator,
            Row::Item(Item::action(app, Action::JumpBack, "Back")),
            Row::Item(Item::action(app, Action::JumpForward, "Forward")),
            Row::Separator,
            Row::Item(Item::action(app, Action::JumpNewest, "Newest change")),
            Row::Item(Item::action(app, Action::AutoJumpToggle, "Auto-jump")),
        ],
        Root::Review => vec![
            Row::Item(Item {
                label: if app.review_list().is_open() {
                    "Close review threads".to_owned()
                } else {
                    "Open review threads".to_owned()
                },
                hint: bindings::hint(Where::View, Action::Review).unwrap_or_default(),
                enabled: true,
                checked: app.review_list().is_open(),
                target: Target::ReviewToggle,
            }),
            Row::Item(Item {
                label: if app.review().resolved {
                    "Hide resolved".to_owned()
                } else {
                    "Show resolved".to_owned()
                },
                hint: "x".to_owned(),
                enabled: app.review_list().is_open() || app.focus() == Focus::ThreadsPane,
                checked: app.review().resolved,
                target: Target::ReviewResolved,
            }),
            Row::Item(Item {
                label: if app.review().file_only {
                    "Show all files".to_owned()
                } else {
                    "Only current file".to_owned()
                },
                hint: "f".to_owned(),
                enabled: app.review_list().is_open(),
                checked: app.review().file_only,
                target: Target::ReviewFile,
            }),
            Row::Separator,
            Row::Item(Item::action(app, Action::NewThread, "New thread")),
            Row::Item(Item::action(app, Action::FileComment, "Comment on file")),
            Row::Item(Item::action(app, Action::Reply, "Reply")),
            Row::Item(Item::action(
                app,
                Action::EditNewestOwn,
                "Edit newest message",
            )),
            Row::Item(Item::action(
                app,
                Action::ToggleResolved,
                "Resolve / reopen",
            )),
        ],
        Root::Diff => vec![
            Row::Item(Item::action(app, Action::DiffHead, "Diff against HEAD")),
            Row::Item(Item::action(
                app,
                Action::DiffSeen,
                "Diff against last seen",
            )),
            Row::Item(Item::action(
                app,
                Action::DiffCheckpoint,
                "Latest checkpoint diff",
            )),
            Row::Item(Item::action(
                app,
                Action::DiffCommit,
                "Diff against commit…",
            )),
            Row::Item(Item::action(app, Action::DiffBase, "Pick base…")),
            Row::Item(Item::action(app, Action::DiffTarget, "Pick target…")),
            Row::Separator,
            Row::Item(Item::action(
                app,
                Action::DiffWhitespace,
                "Ignore whitespace",
            )),
            Row::Separator,
            Row::Item(Item::action(app, Action::CheckpointFile, "Checkpoint file")),
            Row::Item(Item::action(
                app,
                Action::CheckpointWorkspace,
                "Checkpoint workspace",
            )),
            Row::Item(Item::action(app, Action::SeenAll, "Mark all files seen")),
        ],
    }
}

pub(crate) fn submenu_rows(app: &App, submenu: Submenu) -> Vec<Row> {
    match submenu {
        Submenu::Layout => vec![
            Row::Item(Item::action(
                app,
                Action::SidebarToggle,
                if app.sidebar.shown() {
                    "Hide sidebar"
                } else {
                    "Show sidebar"
                },
            )),
            Row::Separator,
            Row::Item(Item::action(
                app,
                Action::TreeToggle,
                if app.sidebar.tree {
                    "Files pane"
                } else {
                    "Show Files pane"
                },
            )),
            Row::Item(Item::action(
                app,
                Action::ThreadsPaneToggle,
                if app.sidebar.threads {
                    "Threads pane"
                } else {
                    "Show Threads pane"
                },
            )),
        ],
        Submenu::Help => vec![
            Row::Item(Item::command(
                "Getting started",
                ":help",
                Target::GettingStarted,
            )),
            Row::Item(Item::command("Doctor", ":doctor", Target::Doctor)),
            Row::Item(Item::command(
                "View keymap",
                "Space ?",
                Target::Action(Action::Help),
            )),
        ],
        Submenu::Go => rows(app, Root::Go),
        Submenu::Review => rows(app, Root::Review),
        Submenu::Diff => rows(app, Root::Diff),
    }
}

fn action_available(app: &App, action: Action) -> bool {
    match action {
        Action::JumpBack => app.jumplist.can_back(),
        Action::JumpForward => app.jumplist.can_forward(),
        Action::JumpNewest => !app.queue().is_empty(),
        Action::NewThread | Action::FileComment => app.has_document() && !app.deleted(),
        Action::Reply | Action::ToggleResolved | Action::EditNewestOwn => {
            app.thread_cursor().thread().is_some()
        }
        Action::DiffHead | Action::DiffCommit => app.has_document() && app.workspace().is_git(),
        Action::DiffSeen => app.has_document() && app.view().has_seen(),
        Action::DiffCheckpoint | Action::CheckpointFile => {
            app.has_document() && app.checkpoints.is_some()
        }
        Action::DiffBase | Action::DiffTarget | Action::DiffWhitespace => app.has_document(),
        Action::CheckpointWorkspace => app.checkpoints.is_some(),
        Action::SeenAll => app.seen.is_some(),
        Action::SidebarToggle => app.sidebar.shown() || app.sidebar.has_restore(),
        _ => true,
    }
}

fn action_checked(app: &App, action: Action) -> bool {
    match action {
        Action::AutoJumpToggle => app.auto_jump(),
        Action::Review => app.review_list().is_open(),
        Action::SidebarToggle => app.sidebar.shown(),
        Action::TreeToggle => app.sidebar.tree,
        Action::ThreadsPaneToggle => app.sidebar.threads,
        Action::DiffHead => app.view().diff().is_some_and(|diff| {
            diff.is_pair(
                &crate::app::diff::Side::Head,
                &crate::app::diff::Side::Working,
            )
        }),
        Action::DiffSeen => app.view().diff().is_some_and(|diff| {
            diff.is_pair(
                &crate::app::diff::Side::Seen,
                &crate::app::diff::Side::Working,
            )
        }),
        Action::DiffCheckpoint => app
            .view()
            .diff()
            .is_some_and(crate::app::diff::DiffView::on_checkpoint),
        Action::DiffCommit => app
            .view()
            .diff()
            .is_some_and(|diff| matches!(diff.base, crate::app::diff::Side::Commit(_))),
        Action::DiffWhitespace => {
            app.compare.whitespace == fathomable_core::diff::Whitespace::Ignore
        }
        _ => false,
    }
}

pub(crate) fn context(app: &App) -> String {
    let branch = app.active_worktree_label().unwrap_or_else(|| {
        app.workspace().root().file_name().map_or_else(
            || app.workspace().root().display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    });
    let subject = if app.getting_started() || !app.has_document() {
        "getting started".to_owned()
    } else if app.review_list().is_open() {
        "review threads".to_owned()
    } else if let Some(directory) = app.directory_path() {
        format!("{}/", directory.display())
    } else {
        let mut path = app.current_path().display().to_string();
        if app.view().changed() {
            path.push_str(" [+]");
        }
        path
    };
    let mut parts = vec![branch, subject];
    if app.review_list().is_open() || app.getting_started() || !app.has_document() {
        return parts.join(" · ");
    }
    if let Some(diff) = app.view().diff() {
        parts.push(diff.header.clone());
    } else if app.view().source_view() {
        parts.push("SOURCE".to_owned());
    }
    parts.join(" · ")
}

pub(crate) fn truncate_left(text: &str, max: usize) -> String {
    if display_width(text) <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() && display_width(&chars.iter().collect::<String>()) + 1 > max {
        chars.remove(0);
    }
    format!("…{}", chars.into_iter().collect::<String>())
}

impl App {
    /// `Space p m`: give the persistent menu bar's row back to content,
    /// or reserve it again.
    pub(crate) fn toggle_menu_bar(&mut self) {
        self.menu_bar.toggle();
        self.relayout();
    }

    pub(crate) fn menu_bar_shown(&self) -> bool {
        self.menu_bar.shown()
    }

    pub(crate) fn title_menu_open(&self) -> bool {
        self.menu_bar.open().is_some()
    }

    pub(crate) fn open_title_menu(&mut self, root: Root) {
        self.take_prefix();
        self.cancel_delete();
        self.park_draft();
        if matches!(
            self.view().mode(),
            crate::app::view::Mode::Command | crate::app::view::Mode::Search { .. }
        ) {
            let _ = self.view_mut().escape();
        }
        self.popup = None;
        self.menu_bar.show(root);
    }

    pub(crate) fn close_title_menu(&mut self) {
        self.menu_bar.close();
    }

    pub(crate) fn run_title_target(&mut self, target: Target) -> Effect {
        self.close_title_menu();
        match target {
            Target::Action(action) => self.act(action),
            Target::ReviewToggle => {
                if self.review_list().is_open() {
                    self.close_review();
                } else {
                    self.open_review();
                }
                Effect::None
            }
            Target::ReviewResolved => {
                self.review_toggle_resolved();
                Effect::None
            }
            Target::ReviewFile => {
                self.review_toggle_file();
                Effect::None
            }
            Target::GettingStarted => {
                self.open_getting_started();
                Effect::None
            }
            Target::Doctor => {
                self.open_doctor();
                Effect::None
            }
            Target::Status => {
                self.open_status();
                Effect::None
            }
            Target::About => {
                self.open_about();
                Effect::None
            }
            Target::Quit => Effect::Quit,
            Target::Submenu(_) => Effect::None,
        }
    }
}

const REVIEW_RESOLVED_KEY: [Chord; 1] = [Chord {
    key: Key::Char('x'),
    ctrl: false,
    alt: false,
}];
const REVIEW_FILE_KEY: [Chord; 1] = [Chord {
    key: Key::Char('f'),
    ctrl: false,
    alt: false,
}];
const STATUS_KEYS: [Chord; 7] = colon("status");
const HELP_KEYS: [Chord; 5] = colon("help");
const DOCTOR_KEYS: [Chord; 7] = colon("doctor");
const ABOUT_KEYS: [Chord; 6] = colon("about");
const QUIT_KEYS: [Chord; 2] = colon("q");

const fn colon<const N: usize>(word: &str) -> [Chord; N] {
    let bytes = word.as_bytes();
    let mut out = [Chord {
        key: Key::Char(':'),
        ctrl: false,
        alt: false,
    }; N];
    let mut index = 1;
    while index < N {
        out[index] = Chord {
            key: Key::Char(bytes[index - 1] as char),
            ctrl: false,
            alt: false,
        };
        index += 1;
    }
    out
}

fn target_keys(target: Target) -> Option<&'static [Chord]> {
    match target {
        Target::Action(action) => bindings::first_keys(Where::View, action),
        Target::ReviewToggle => bindings::first_keys(Where::View, Action::Review),
        Target::ReviewResolved => Some(&REVIEW_RESOLVED_KEY),
        Target::ReviewFile => Some(&REVIEW_FILE_KEY),
        Target::GettingStarted => Some(&HELP_KEYS),
        Target::Doctor => Some(&DOCTOR_KEYS),
        Target::Status => Some(&STATUS_KEYS),
        Target::About => Some(&ABOUT_KEYS),
        Target::Quit => Some(&QUIT_KEYS),
        Target::Submenu(_) => None,
    }
}

fn selected_target(app: &App) -> Option<Target> {
    let open = app.menu_bar.open()?;
    match open.focused {
        Focused::Label => None,
        Focused::Root(index) => rows(app, open.root)
            .get(index)?
            .item()
            .filter(|item| item.enabled)
            .map(|item| item.target),
        Focused::Child(index) => {
            let (submenu, _) = open.submenu?;
            submenu_rows(app, submenu)
                .get(index)?
                .item()
                .filter(|item| item.enabled)
                .map(|item| item.target)
        }
    }
}

fn focus_root_item(app: &mut App, index: usize, open_submenu: bool) {
    let rows = app
        .menu_bar
        .open()
        .map(|open| rows(app, open.root))
        .unwrap_or_default();
    let target = rows.get(index).and_then(Row::item).map(|item| item.target);
    let Some(open) = app.menu_bar.open_mut() else {
        return;
    };
    open.focused = Focused::Root(index);
    open.typed.clear();
    let layout = layout_at(
        &rows,
        open.root.title(),
        0,
        MENU_TOP,
        open.root_scroll,
        app.width,
        app.height,
    );
    keep_visible(
        &mut open.root_scroll,
        index,
        layout.visible_rows,
        rows.len(),
    );
    open.submenu = None;
    if open_submenu && let Some(Target::Submenu(submenu)) = target {
        open_submenu_at(app, submenu, index);
    }
}

fn open_submenu_at(app: &mut App, submenu: Submenu, parent: usize) {
    let latest_start = app.height.saturating_sub(STATUS_ROWS + BORDER_ROWS + 1);
    let Some(open) = app.menu_bar.open_mut() else {
        return;
    };
    open.submenu = Some((submenu, parent));
    open.child_scroll = 0;
    let parent_y = MENU_TOP + 1 + parent.saturating_sub(open.root_scroll);
    if parent_y > latest_start {
        open.root_scroll = open.root_scroll.saturating_add(parent_y - latest_start);
    }
}

fn focus_child_item(app: &mut App, index: usize) {
    let Some(opened) = app.menu_bar.open().cloned() else {
        return;
    };
    let Some((submenu, _)) = opened.submenu else {
        return;
    };
    let child_rows = submenu_rows(app, submenu);
    let root_rows = rows(app, opened.root);
    let visible = root_layout(app, &root_rows)
        .and_then(|root| child_layout(app, root, &root_rows, &child_rows))
        .map_or(1, |layout| layout.visible_rows.max(1));
    let Some(open) = app.menu_bar.open_mut() else {
        return;
    };
    open.focused = Focused::Child(index);
    open.typed.clear();
    keep_visible(&mut open.child_scroll, index, visible, child_rows.len());
}

fn switch_root(app: &mut App, delta: isize) {
    let Some(open) = app.menu_bar.open() else {
        return;
    };
    let shown: Vec<Root> = labels(app.width)
        .into_iter()
        .map(|label| label.root)
        .collect();
    let at = shown
        .iter()
        .position(|root| *root == open.root)
        .unwrap_or(0);
    let next = at
        .saturating_add_signed(delta)
        .min(shown.len().saturating_sub(1));
    app.menu_bar.show(shown[next]);
}

fn shortcut(app: &mut App, open: &Open, chord: Chord) -> Option<Effect> {
    let root_rows = rows(app, open.root);
    let child_rows = open
        .submenu
        .map(|(submenu, _)| submenu_rows(app, submenu))
        .unwrap_or_default();
    let mut typed = open.typed.clone();
    typed.push(chord);
    let candidates = root_rows
        .iter()
        .chain(child_rows.iter())
        .filter_map(Row::item)
        .filter(|item| item.enabled)
        .filter_map(|item| target_keys(item.target).map(|keys| (item.target, keys)));
    let mut prefix = false;
    for (target, keys) in candidates {
        if keys == typed {
            return Some(app.run_title_target(target));
        }
        prefix |= keys.len() > typed.len() && keys.starts_with(&typed);
    }
    if prefix {
        if let Some(state) = app.menu_bar.open_mut() {
            state.typed = typed;
        }
        return Some(Effect::None);
    }
    if !open.typed.is_empty() {
        app.close_title_menu();
        return Some(Effect::None);
    }
    None
}

/// Keyboard navigation while a title-bar menu is open.
#[expect(
    clippy::too_many_lines,
    reason = "the menu's small keyboard state machine is clearest in one match"
)]
pub(crate) fn key(app: &mut App, event: KeyEvent) -> Effect {
    let Some(open) = app.menu_bar.open().cloned() else {
        return Effect::None;
    };
    let plain = event.modifiers.is_empty() || event.modifiers == KeyModifiers::SHIFT;
    if !open.typed.is_empty()
        && let Some(chord) = Chord::from_event(event)
        && let Some(effect) = shortcut(app, &open, chord)
    {
        return effect;
    }
    match event.code {
        KeyCode::Esc => app.close_title_menu(),
        KeyCode::Left | KeyCode::Char('h') if plain => match open.focused {
            Focused::Label => switch_root(app, -1),
            Focused::Child(_) => {
                if let Some(state) = app.menu_bar.open_mut()
                    && let Some((_, parent)) = state.submenu.take()
                {
                    state.focused = Focused::Root(parent);
                    state.child_scroll = 0;
                }
            }
            Focused::Root(_) => {}
        },
        KeyCode::Right | KeyCode::Char('l') if plain => match open.focused {
            Focused::Label => switch_root(app, 1),
            Focused::Root(index) => {
                let target = rows(app, open.root)
                    .get(index)
                    .and_then(Row::item)
                    .map(|item| item.target);
                if let Some(Target::Submenu(submenu)) = target {
                    let child = submenu_rows(app, submenu);
                    if let Some(first) = next_item(&child, None, 1) {
                        open_submenu_at(app, submenu, index);
                        focus_child_item(app, first);
                    }
                }
            }
            Focused::Child(_) => {}
        },
        KeyCode::Down | KeyCode::Char('j') if plain => match open.focused {
            Focused::Label => {
                let root_rows = rows(app, open.root);
                if let Some(first) = next_item(&root_rows, None, 1) {
                    focus_root_item(app, first, false);
                }
            }
            Focused::Root(index) => {
                let root_rows = rows(app, open.root);
                if let Some(next) = next_item(&root_rows, Some(index), 1) {
                    focus_root_item(app, next, false);
                }
            }
            Focused::Child(index) => {
                if let Some((submenu, _)) = open.submenu {
                    let child = submenu_rows(app, submenu);
                    if let Some(next) = next_item(&child, Some(index), 1) {
                        focus_child_item(app, next);
                    }
                }
            }
        },
        KeyCode::Up | KeyCode::Char('k') if plain => match open.focused {
            Focused::Label => {}
            Focused::Root(index) => {
                let root_rows = rows(app, open.root);
                if let Some(previous) = next_item(&root_rows, Some(index), -1) {
                    focus_root_item(app, previous, false);
                } else if let Some(state) = app.menu_bar.open_mut() {
                    state.focused = Focused::Label;
                    state.submenu = None;
                }
            }
            Focused::Child(index) => {
                if let Some((submenu, _)) = open.submenu {
                    let child = submenu_rows(app, submenu);
                    if let Some(previous) = next_item(&child, Some(index), -1) {
                        focus_child_item(app, previous);
                    }
                }
            }
        },
        KeyCode::Enter => {
            if let Some(target) = selected_target(app) {
                if let Target::Submenu(submenu) = target {
                    let parent = match open.focused {
                        Focused::Root(index) => index,
                        Focused::Label | Focused::Child(_) => return Effect::None,
                    };
                    let child = submenu_rows(app, submenu);
                    if let Some(first) = next_item(&child, None, 1) {
                        open_submenu_at(app, submenu, parent);
                        focus_child_item(app, first);
                    }
                    return Effect::None;
                }
                return app.run_title_target(target);
            }
        }
        _ => {
            let Some(chord) = Chord::from_event(event) else {
                return Effect::None;
            };
            if let Some(effect) = shortcut(app, &open, chord) {
                return effect;
            }
            app.close_title_menu();
        }
    }
    Effect::None
}

fn scroll_menu(app: &mut App, child: bool, delta: isize) {
    let Some(open) = app.menu_bar.open().cloned() else {
        return;
    };
    let root_rows = rows(app, open.root);
    let Some(root) = root_layout(app, &root_rows) else {
        return;
    };
    let (rows, visible) = if child {
        let Some((submenu, _)) = open.submenu else {
            return;
        };
        let rows = submenu_rows(app, submenu);
        let visible =
            child_layout(app, root, &root_rows, &rows).map_or(0, |layout| layout.visible_rows);
        (rows, visible)
    } else {
        (root_rows, root.visible_rows)
    };
    let Some(state) = app.menu_bar.open_mut() else {
        return;
    };
    if child {
        state.child_scroll = state
            .child_scroll
            .saturating_add_signed(delta)
            .min(rows.len().saturating_sub(visible.max(1)));
        let end = (state.child_scroll + visible.max(1)).min(rows.len());
        let selected = match state.focused {
            Focused::Child(index)
                if index >= state.child_scroll
                    && index < end
                    && rows
                        .get(index)
                        .and_then(Row::item)
                        .is_some_and(|item| item.enabled) =>
            {
                Some(index)
            }
            Focused::Label | Focused::Root(_) | Focused::Child(_) => (state.child_scroll..end)
                .find(|index| {
                    rows.get(*index)
                        .and_then(Row::item)
                        .is_some_and(|item| item.enabled)
                }),
        };
        if let Some(index) = selected {
            state.focused = Focused::Child(index);
        } else if let Some((_, parent)) = state.submenu.take() {
            state.focused = Focused::Root(parent);
        }
        return;
    }
    state.root_scroll = state
        .root_scroll
        .saturating_add_signed(delta)
        .min(rows.len().saturating_sub(visible.max(1)));
    state.submenu = None;
    state.child_scroll = 0;
    let end = (state.root_scroll + visible.max(1)).min(rows.len());
    let selected = match state.focused {
        Focused::Root(index) if index >= state.root_scroll && index < end => Some(index),
        Focused::Label | Focused::Root(_) | Focused::Child(_) => {
            (state.root_scroll..end).find(|index| {
                rows.get(*index)
                    .and_then(Row::item)
                    .is_some_and(|item| item.enabled)
            })
        }
    };
    state.focused = selected.map_or(Focused::Label, Focused::Root);
}

/// The title bar and its open menu's share of one mouse event. `None`
/// lets an event outside a closed menu continue to the panes.
pub(crate) fn mouse(app: &mut App, event: MouseEvent) -> Option<Effect> {
    if !app.menu_bar.shown() {
        return None;
    }
    let column = usize::from(event.column);
    let row = usize::from(event.row);
    let left = event.kind == MouseEventKind::Down(MouseButton::Left);
    let right = event.kind == MouseEventKind::Down(MouseButton::Right);
    if row == 0 {
        if let Some(root) = label_at(app.width, column) {
            match event.kind {
                MouseEventKind::Moved
                    if app.menu_bar.open().is_some_and(|open| open.root != root) =>
                {
                    app.open_title_menu(root);
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if app.menu_bar.open().is_some_and(|open| open.root == root) {
                        app.close_title_menu();
                    } else {
                        app.open_title_menu(root);
                    }
                }
                _ => {}
            }
        } else if left {
            app.close_title_menu();
        }
        return Some(Effect::None);
    }
    let open = app.menu_bar.open().cloned()?;
    let root_rows = rows(app, open.root);
    let root = root_layout(app, &root_rows)?;
    let child_rows = open
        .submenu
        .map(|(submenu, _)| submenu_rows(app, submenu))
        .unwrap_or_default();
    let child = child_layout(app, root, &root_rows, &child_rows);

    if matches!(
        event.kind,
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
    ) {
        let delta = if event.kind == MouseEventKind::ScrollDown {
            1
        } else {
            -1
        };
        if child.is_some_and(|layout| layout.contains(column, row)) {
            scroll_menu(app, true, delta);
        } else if root.contains(column, row) {
            scroll_menu(app, false, delta);
        }
        return Some(Effect::None);
    }

    if let Some(layout) = child
        && let Some(index) = layout.row_at(column, row, &child_rows)
    {
        focus_child_item(app, index);
        if left
            && let Some(item) = child_rows.get(index).and_then(Row::item)
            && item.enabled
        {
            return Some(app.run_title_target(item.target));
        }
        return Some(Effect::None);
    }
    if child.is_some_and(|layout| layout.contains(column, row)) {
        return Some(Effect::None);
    }
    if let Some(index) = root.row_at(column, row, &root_rows) {
        focus_root_item(app, index, true);
        if left
            && let Some(item) = root_rows.get(index).and_then(Row::item)
            && item.enabled
        {
            if matches!(item.target, Target::Submenu(_)) {
                return Some(Effect::None);
            }
            return Some(app.run_title_target(item.target));
        }
        return Some(Effect::None);
    }
    if matches!(event.kind, MouseEventKind::Moved)
        && (root.contains(column, row) || child.is_some_and(|layout| layout.contains(column, row)))
    {
        return Some(Effect::None);
    }
    if left {
        app.close_title_menu();
        return Some(Effect::None);
    }
    if right {
        app.close_title_menu();
        return None;
    }
    Some(Effect::None)
}

/// A resize closes the transient menu so changed label and wrapping geometry
/// cannot leave an invisible selection active.
pub(crate) fn resize(app: &mut App) {
    if app.title_menu_open() {
        app.close_title_menu();
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Context;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
    use fathomable_core::config::SidebarConfig;

    use super::{
        Focused, Root, Submenu, child_layout, context, labels, root_layout, rows, submenu_rows,
    };
    use crate::app::input::{keys, mouse};
    use crate::app::testing;

    #[test]
    fn labels_collapse_to_the_application_menu_when_narrow() {
        assert_eq!(labels(80).len(), 4);
        assert_eq!(labels(20).len(), 1);
    }

    #[test]
    fn menus_are_stable_and_context_names_the_document() -> anyhow::Result<()> {
        let dir = testing::workspace("menu-bar-model", testing::README)?;
        let app = testing::app(&dir)?;
        assert_eq!(rows(&app, Root::App).len(), 6);
        assert_eq!(rows(&app, Root::Go).len(), 9);
        assert_eq!(rows(&app, Root::Review).len(), 9);
        assert_eq!(rows(&app, Root::Diff).len(), 12);
        assert_eq!(submenu_rows(&app, super::Submenu::Help).len(), 3);
        assert!(
            !rows(&app, Root::App)
                .iter()
                .filter_map(super::Row::item)
                .any(|item| item.target == super::Target::Action(super::Action::MenuBarToggle)),
            "the menu bar can only be hidden through Space p m"
        );
        assert!(context(&app).contains("README.md"));
        Ok(())
    }

    fn shown_app(name: &str) -> anyhow::Result<crate::app::App> {
        let dir = testing::workspace(name, testing::README)?;
        testing::AppBuilder::new(&dir)
            .options(|mut options| {
                options.menu_bar = true;
                options.sidebar = SidebarConfig::default();
                options
            })
            .build()
    }

    #[test]
    fn menu_bar_reserves_a_row_and_draws_passive_identity() -> anyhow::Result<()> {
        let app = shown_app("menu-bar-draw")?;
        assert_eq!(app.pane_top(), 1);
        assert_eq!(app.pane_rows(), 28);
        let screen = testing::screen(&app)?;
        assert!(screen[0].contains("☰") && screen[0].contains("Go  Review  Diff"));
        assert!(screen[0].ends_with("ws · README.md"), "{:?}", screen[0]);
        Ok(())
    }

    #[test]
    fn mouse_and_keys_walk_root_labels_and_an_aligned_submenu() -> anyhow::Result<()> {
        let mut app = shown_app("menu-bar-navigation")?;
        testing::click(&mut app, 4, 0);
        assert!(
            app.menu_bar
                .open()
                .is_some_and(|open| open.root == Root::Go)
        );
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(matches!(
            app.menu_bar.open().map(|open| open.focused),
            Some(Focused::Root(0))
        ));
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(
            app.menu_bar
                .open()
                .is_some_and(|open| open.root == Root::Review)
        );

        testing::click(&mut app, 1, 0);
        assert!(
            testing::screen(&app)?[1].starts_with("╭ Fathomable "),
            "the menu is attached below the bar with a rounded titled border"
        );
        let root_rows = rows(&app, Root::App);
        let root = root_layout(&app, &root_rows).context("root menu")?;
        mouse::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 2,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        let child_rows = submenu_rows(&app, Submenu::Layout);
        let child = child_layout(&app, root, &root_rows, &child_rows).context("layout submenu")?;
        assert_eq!(child.y, root.y + 1, "submenu aligns with Layout row");
        Ok(())
    }

    #[test]
    fn whole_sidebar_toggle_restores_the_pane_composition() -> anyhow::Result<()> {
        let mut app = shown_app("sidebar-restore")?;
        assert!(app.sidebar.tree && app.sidebar.threads);
        testing::press(&mut app, " pf");
        assert!(!app.sidebar.tree && app.sidebar.threads);
        testing::press(&mut app, " ps");
        assert!(!app.sidebar.tree && !app.sidebar.threads);
        testing::press(&mut app, " ps");
        assert!(!app.sidebar.tree && app.sidebar.threads);

        app.sidebar = crate::app::sidebar::Sidebar::new(SidebarConfig {
            visible: true,
            files: false,
            threads: false,
            ..SidebarConfig::default()
        });
        testing::press(&mut app, " ps");
        assert_eq!(
            app.message(),
            Some("sidebar has no panes; show Files or Threads first")
        );
        Ok(())
    }

    #[test]
    fn hiding_the_menu_bar_returns_its_row_and_identity_to_status() -> anyhow::Result<()> {
        let mut app = shown_app("menu-bar-hide")?;
        testing::press(&mut app, " pm");
        assert!(!app.menu_bar_shown());
        assert_eq!(app.pane_top(), 0);
        assert_eq!(app.pane_rows(), 29);
        let screen = testing::screen(&app)?;
        assert!(screen[29].contains("README.md"), "{:?}", screen[29]);
        Ok(())
    }

    #[test]
    fn displayed_multi_chord_runs_from_an_open_title_menu() -> anyhow::Result<()> {
        let mut app = shown_app("menu-bar-shortcut")?;
        testing::click(&mut app, 4, 0);
        testing::press(&mut app, " f");
        assert!(matches!(
            app.popup(),
            Some(crate::app::Popup::Picker(picker))
                if picker.kind() == crate::app::PickerKind::Files
        ));
        assert!(!app.title_menu_open());
        Ok(())
    }

    #[test]
    fn narrow_application_menu_contains_the_hidden_workflow_menus() -> anyhow::Result<()> {
        let dir = testing::workspace("menu-bar-narrow", testing::README)?;
        let app = testing::AppBuilder::new(&dir)
            .width(20)
            .options(|mut options| {
                options.menu_bar = true;
                options
            })
            .build()?;
        let labels: Vec<String> = rows(&app, Root::App)
            .iter()
            .filter_map(super::Row::item)
            .map(|item| item.label.clone())
            .collect();
        assert!(
            labels
                .windows(3)
                .any(|labels| labels == ["Go", "Review", "Diff"])
        );
        Ok(())
    }

    #[test]
    fn getting_started_closes_before_a_thread_draft_opens() -> anyhow::Result<()> {
        let mut app = shown_app("getting-started-action")?;
        app.open_getting_started();
        testing::press(&mut app, " cf");
        assert!(!app.getting_started());
        assert!(matches!(app.popup(), Some(crate::app::Popup::Compose(_))));
        Ok(())
    }

    #[test]
    fn short_submenu_keeps_one_item_visible_and_aligned() -> anyhow::Result<()> {
        let mut app = shown_app("short-submenu")?;
        app.resize(100, 6);
        testing::click(&mut app, 1, 0);
        mouse::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 2,
                row: 3,
                modifiers: KeyModifiers::NONE,
            },
        );
        let root_rows = rows(&app, Root::App);
        let root = root_layout(&app, &root_rows).context("root menu")?;
        let child_rows = submenu_rows(&app, Submenu::Help);
        let child = child_layout(&app, root, &root_rows, &child_rows).context("help submenu")?;
        assert!(child.visible_rows >= 1);
        assert_eq!(
            child.y,
            root.y + 1 + 1usize.saturating_sub(root.scroll),
            "the child border stays aligned with the scrolled parent row"
        );

        app.close_title_menu();
        testing::click(&mut app, 1, 0);
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        let root_rows = rows(&app, Root::App);
        let root = root_layout(&app, &root_rows).context("keyboard root menu")?;
        let child_rows = submenu_rows(&app, Submenu::Help);
        let child =
            child_layout(&app, root, &root_rows, &child_rows).context("keyboard help submenu")?;
        assert!(child.visible_rows >= 1);
        Ok(())
    }

    #[test]
    fn submenu_border_consumes_clicks_over_the_parent() -> anyhow::Result<()> {
        let dir = testing::workspace("submenu-overlap", testing::README)?;
        let mut app = testing::AppBuilder::new(&dir)
            .width(40)
            .options(|mut options| {
                options.menu_bar = true;
                options
            })
            .build()?;
        testing::click(&mut app, 1, 0);
        mouse::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 2,
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        let root_rows = rows(&app, Root::App);
        let root = root_layout(&app, &root_rows).context("root menu")?;
        let child_rows = submenu_rows(&app, Submenu::Layout);
        let child = child_layout(&app, root, &root_rows, &child_rows).context("layout submenu")?;
        let effect = mouse::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
                column: u16::try_from(child.x + 3)?,
                row: u16::try_from(child.y + child.height - 1)?,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert_eq!(effect, crate::app::view::Effect::None);
        assert!(app.title_menu_open());
        Ok(())
    }

    #[test]
    fn scrolling_the_root_closes_a_hidden_submenu_selection() -> anyhow::Result<()> {
        let mut app = shown_app("submenu-scroll")?;
        app.resize(100, 8);
        testing::click(&mut app, 1, 0);
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(matches!(
            app.menu_bar.open().map(|open| open.focused),
            Some(Focused::Child(_))
        ));
        mouse::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 2,
                row: 6,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(app.menu_bar.open().is_some_and(|open| {
            open.submenu.is_none() && !matches!(open.focused, Focused::Child(_))
        }));
        let sidebar = app.sidebar.shown();
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.sidebar.shown(), sidebar);
        Ok(())
    }

    #[test]
    fn scrolling_a_child_moves_focus_to_a_visible_item() -> anyhow::Result<()> {
        let mut app = shown_app("child-scroll")?;
        app.resize(100, 8);
        testing::click(&mut app, 1, 0);
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(matches!(
            app.menu_bar.open().map(|open| open.focused),
            Some(Focused::Child(0))
        ));
        let root_rows = rows(&app, Root::App);
        let root = root_layout(&app, &root_rows).context("root menu")?;
        let child_rows = submenu_rows(&app, Submenu::Layout);
        let child = child_layout(&app, root, &root_rows, &child_rows).context("layout submenu")?;
        mouse::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: u16::try_from(child.x + 2)?,
                row: u16::try_from(child.y + 1)?,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(matches!(
            app.menu_bar.open().map(|open| open.focused),
            Some(Focused::Child(index)) if index >= 2
        ));
        Ok(())
    }

    #[test]
    fn menu_width_preserves_full_shortcuts_and_colon_commands_run() -> anyhow::Result<()> {
        let mut app = shown_app("menu-width")?;
        testing::click(&mut app, 4, 0);
        assert!(
            testing::screen(&app)?
                .iter()
                .any(|row| row.contains("Space F i")),
            "the longest Go hint is not clipped"
        );
        app.close_title_menu();
        testing::click(&mut app, 1, 0);
        testing::press(&mut app, ":about");
        assert!(matches!(app.popup(), Some(crate::app::Popup::About)));
        Ok(())
    }

    #[test]
    fn review_menu_closes_an_open_unfocused_review() -> anyhow::Result<()> {
        let mut app = shown_app("review-close")?;
        app.open_review();
        app.window_files();
        assert!(app.review_list().is_open());
        app.run_title_target(super::Target::ReviewToggle);
        assert!(!app.review_list().is_open());
        Ok(())
    }

    #[test]
    fn enter_does_not_run_a_disabled_hovered_item() -> anyhow::Result<()> {
        let mut app = shown_app("disabled-enter")?;
        testing::click(&mut app, 10, 0);
        mouse::handle_mouse(
            &mut app,
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 12,
                row: 3,
                modifiers: KeyModifiers::NONE,
            },
        );
        assert!(matches!(
            app.menu_bar.open().map(|open| open.focused),
            Some(Focused::Root(1))
        ));
        keys::handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.review().resolved);
        assert!(app.title_menu_open());
        Ok(())
    }

    #[test]
    fn resize_closes_a_workflow_menu_whose_label_collapses() -> anyhow::Result<()> {
        let mut app = shown_app("menu-resize")?;
        testing::click(&mut app, 4, 0);
        assert!(app.title_menu_open());
        app.resize(20, 30);
        assert!(!app.title_menu_open());
        Ok(())
    }
}
