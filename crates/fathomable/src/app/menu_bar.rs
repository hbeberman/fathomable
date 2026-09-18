// @okf-doc: /decisions/0081-the-menu-bar.md
//! The persistent menu bar: its stable workflow menus, comparison endpoints,
//! one-level fly-outs, geometry, and mouse/keyboard navigation.

use fathomable_core::config::DiffMode;
use fathomable_core::layout::display_width;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::input::bindings::{self, Action, Chord, Key, Where};
use crate::app::view::Effect;
use crate::app::{App, Focus, PickerKind};

const FULL_LABEL_WIDTH: usize = 29;
const TAIL_GAP: usize = 2;
const MENU_TOP: usize = 1;
const STATUS_ROWS: usize = 1;
const BORDER_ROWS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Root {
    App,
    Layout,
    Go,
    Review,
    Diff,
}

impl Root {
    pub(crate) const ALL: [Self; 5] = [Self::App, Self::Layout, Self::Go, Self::Review, Self::Diff];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::App => "☰",
            Self::Layout => "Layout",
            Self::Go => "Go",
            Self::Review => "Review",
            Self::Diff => "Diff",
        }
    }

    pub(crate) const fn title(self) -> &'static str {
        match self {
            Self::App => "Fathomable",
            Self::Layout => "Layout",
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
    ReviewResolved,
    ReviewFile,
    GettingStarted,
    Doctor,
    Licenses,
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
    /// This row is the selected member of a mutually exclusive choice.
    pub(crate) active: bool,
    pub(crate) target: Target,
}

impl Item {
    fn action(app: &App, action: Action, label: impl Into<String>) -> Self {
        let target = Target::Action(action);
        Self {
            label: label.into(),
            hint: target_keys(target)
                .map(bindings::menu_spell)
                .unwrap_or_default(),
            enabled: action_available(app, action),
            checked: action_checked(app, action),
            active: false,
            target,
        }
    }

    fn command(label: &'static str, target: Target) -> Self {
        Self {
            label: label.to_owned(),
            hint: target_keys(target)
                .map(bindings::menu_spell)
                .unwrap_or_default(),
            enabled: true,
            checked: false,
            active: false,
            target,
        }
    }

    fn submenu(label: &'static str, submenu: Submenu) -> Self {
        Self::command(label, Target::Submenu(submenu))
    }

    fn choice(app: &App, action: Action, label: &'static str, active: bool) -> Self {
        Self {
            active,
            checked: false,
            ..Self::action(app, action, label)
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BarLabel {
    pub(crate) text: String,
    pub(crate) x: usize,
    pub(crate) width: usize,
    pub(crate) picker: PickerKind,
}

impl BarLabel {
    pub(crate) fn contains(&self, column: usize) -> bool {
        column >= self.x && column < self.x + self.width
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BarTail {
    pub(crate) padding: usize,
    pub(crate) base: Option<BarLabel>,
    pub(crate) target: Option<BarLabel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BarIdentity {
    pub(crate) repo: String,
    pub(crate) x: usize,
    pub(crate) width: usize,
    pub(crate) picker: Option<PickerKind>,
}

impl BarIdentity {
    pub(crate) fn repo_contains(&self, column: usize) -> bool {
        column >= self.x && column < self.x + display_width(&self.repo)
    }

    fn picker_at(&self, column: usize) -> Option<PickerKind> {
        self.repo_contains(column).then_some(self.picker).flatten()
    }
}

impl BarTail {
    pub(crate) fn picker_at(&self, column: usize) -> Option<PickerKind> {
        [&self.base, &self.target]
            .into_iter()
            .flatten()
            .find(|label| label.contains(column))
            .map(|label| label.picker)
    }

    pub(crate) const fn endpoints_rendered(&self) -> bool {
        self.target.is_some()
    }
}

/// Lay out the right-aligned comparison buttons after the root labels.
pub(crate) fn bar_tail(app: &App, width: usize) -> BarTail {
    let labels = labels(width);
    let used = labels
        .last()
        .map_or(0, |label| label.x.saturating_add(label.width));
    let available = width.saturating_sub(used);
    let (base, target) = app.comparison_menu_pair();
    let base_width = display_width(&base);
    let target_width = display_width(&target);
    let off = app.diff_mode() == DiffMode::Off;
    let tail_width = if off {
        target_width
    } else {
        base_width + 4 + target_width
    };
    let show_tail = tail_width + TAIL_GAP <= available;
    let padding = available.saturating_sub(usize::from(show_tail) * tail_width);
    let tail_x = used + padding;
    BarTail {
        padding,
        base: (show_tail && !off).then_some(BarLabel {
            text: base,
            x: tail_x,
            width: base_width,
            picker: PickerKind::ComparisonBase,
        }),
        target: show_tail.then_some(BarLabel {
            text: target,
            x: if off { tail_x } else { tail_x + base_width + 4 },
            width: target_width,
            picker: PickerKind::ComparisonTarget,
        }),
    }
}

/// Lay out the centered repository and active worktree.
pub(crate) fn bar_identity(app: &App, width: usize) -> Option<BarIdentity> {
    let left_end = labels(width)
        .last()
        .map_or(0, |label| label.x.saturating_add(label.width));
    let tail = bar_tail(app, width);
    let right_start = tail
        .base
        .as_ref()
        .or(tail.target.as_ref())
        .map_or(width, |label| label.x);
    identity_within(
        app,
        width,
        left_end.saturating_add(TAIL_GAP),
        right_start.saturating_sub(TAIL_GAP),
    )
}

fn identity_within(
    app: &App,
    width: usize,
    left_bound: usize,
    right_bound: usize,
) -> Option<BarIdentity> {
    let left_room = width.saturating_sub(left_bound.saturating_mul(2));
    let right_room = right_bound
        .saturating_mul(2)
        .saturating_add(1)
        .saturating_sub(width);
    let max_width = left_room.min(right_room);
    let repo = app.workspace().root().file_name().map_or_else(
        || "/".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    let worktree = app.worktree_label();
    let repo = worktree
        .as_ref()
        .map_or(repo.clone(), |branch| format!("{repo} · {branch}"));
    let fitted_repo = super::draw::fit_ellipsis(&repo, max_width)
        .trim_end()
        .to_owned();
    if fitted_repo.is_empty() || fitted_repo == "…" {
        return None;
    }
    let identity_width = display_width(&fitted_repo);
    Some(BarIdentity {
        repo: fitted_repo,
        x: width.saturating_sub(identity_width) / 2,
        width: identity_width,
        picker: worktree.map(|_| PickerKind::Worktree),
    })
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
            let mut rows = Vec::new();
            if labels(app.width).len() == 1 {
                rows.extend([
                    Row::Item(Item::submenu("Layout", Submenu::Layout)),
                    Row::Item(Item::submenu("Go", Submenu::Go)),
                    Row::Item(Item::submenu("Review", Submenu::Review)),
                    Row::Item(Item::submenu("Diff", Submenu::Diff)),
                ]);
            }
            rows.push(Row::Item(Item::submenu("Help", Submenu::Help)));
            rows.extend([
                Row::Separator,
                Row::Item(Item::command("Status", Target::Status)),
                Row::Item(Item::command("About", Target::About)),
                Row::Item(Item::command("Quit", Target::Quit)),
            ]);
            rows
        }
        Root::Layout => submenu_rows(app, Submenu::Layout),
        Root::Go => vec![
            Row::Item(Item::action(app, Action::PickFile, "Open file…")),
            Row::Item(Item::action(app, Action::PickAnyFile, "Open w/ ignored…")),
            Row::Item(Item::action(app, Action::PickRecent, "Recent files…")),
            Row::Separator,
            Row::Item(Item::action(app, Action::JumpBack, "Back")),
            Row::Item(Item::action(app, Action::JumpForward, "Forward")),
        ],
        Root::Review => vec![
            Row::Item(Item::action(app, Action::Review, "Reviews")),
            Row::Item(Item::action(
                app,
                Action::ReviewRecentlyResolved,
                "Recently resolved",
            )),
            Row::Item(Item::action(
                app,
                Action::ReviewArchived,
                "Archived threads",
            )),
            Row::Item(Item::action(
                app,
                Action::ArchiveResolved,
                "Archive resolved threads",
            )),
            Row::Item(Item::action(app, Action::ClearBoard, "Clear board…")),
            Row::Item(Item {
                label: "Show resolved".to_owned(),
                hint: "x".to_owned(),
                enabled: app.review_list().is_open() || app.focus() == Focus::ThreadsPane,
                checked: app.review().resolved,
                active: false,
                target: Target::ReviewResolved,
            }),
            Row::Item(Item {
                label: "Only current file".to_owned(),
                hint: "s".to_owned(),
                enabled: app.review_list().is_open(),
                checked: app.review().file_only,
                active: false,
                target: Target::ReviewFile,
            }),
            Row::Separator,
            Row::Item(Item::action(app, Action::NewThread, "New thread")),
            Row::Item(Item::action(app, Action::FileComment, "File comment")),
            Row::Item(Item::action(app, Action::Reply, "Reply")),
            Row::Item(Item::action(
                app,
                Action::EditNewestOwn,
                "Edit newest message",
            )),
            Row::Item(Item::action(
                app,
                Action::ToggleAutoResolve,
                "Toggle auto-resolve",
            )),
            Row::Item(Item::action(
                app,
                Action::ToggleResolved,
                "Resolve / reopen",
            )),
            Row::Item(Item::action(app, Action::ArchiveThread, "Archive thread")),
            Row::Item(Item::action(app, Action::RestoreThread, "Restore thread")),
        ],
        Root::Diff => vec![
            Row::Item(Item::choice(
                app,
                Action::DiffStandard,
                "Standard diff",
                app.diff_mode() == DiffMode::Standard,
            )),
            Row::Item(Item::choice(
                app,
                Action::DiffUnified,
                "Unified diff",
                app.diff_mode() == DiffMode::Unified,
            )),
            Row::Item(Item::choice(
                app,
                Action::DiffOff,
                "Diff off",
                app.diff_mode() == DiffMode::Off,
            )),
            Row::Separator,
            Row::Item(Item::action(app, Action::ComparisonBase, "Pick base…")),
            Row::Item(Item::action(app, Action::ComparisonTarget, "Pick target…")),
            Row::Item(Item::action(
                app,
                Action::ComparisonHeadWorkingTree,
                "Head to WorkingTree",
            )),
            Row::Separator,
            Row::Item(Item::action(
                app,
                Action::ComparisonSave,
                "Save review point",
            )),
            Row::Separator,
            Row::Item(Item::action(
                app,
                Action::ComparisonWhitespace,
                "Ignore whitespace",
            )),
        ],
    }
}

pub(crate) fn submenu_rows(app: &App, submenu: Submenu) -> Vec<Row> {
    match submenu {
        Submenu::Layout => vec![
            Row::Item(Item::choice(
                app,
                Action::FileView,
                "File view",
                !app.review_list().is_open(),
            )),
            Row::Item(Item::choice(
                app,
                Action::Review,
                "Reviews view",
                app.review_list().is_open(),
            )),
            Row::Separator,
            Row::Item(Item::action(app, Action::SidebarToggle, "Sidebar")),
            Row::Item(Item::action(app, Action::TreeToggle, "Files pane")),
            Row::Item(Item::action(app, Action::ThreadsPaneToggle, "Threads pane")),
        ],
        Submenu::Help => vec![
            Row::Item(Item::command("Getting started", Target::GettingStarted)),
            Row::Item(Item::command("Doctor", Target::Doctor)),
            Row::Item(Item::command("View keymap", Target::Action(Action::Help))),
            Row::Item(Item::command("Licenses", Target::Licenses)),
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
        Action::NewThread | Action::FileComment => app.has_document() && !app.deleted(),
        Action::Reply | Action::ToggleResolved | Action::EditNewestOwn => {
            app.thread_cursor().thread().is_some()
        }
        Action::ToggleAutoResolve => app
            .thread_cursor()
            .thread()
            .and_then(|id| app.thread(id))
            .is_some_and(|thread| {
                thread.lifecycle() != fathomable_core::annotations::Lifecycle::Resolved
            }),
        Action::ComparisonSave => app.review_points.is_some(),
        Action::ComparisonWhitespace | Action::FilesChanged => app.diff_mode() != DiffMode::Off,
        Action::SourceView => app.source_view_available(),
        Action::ArchiveThread => app
            .thread_cursor()
            .thread()
            .and_then(|id| app.thread(id))
            .is_some_and(|thread| {
                !thread.is_archived()
                    && thread.status() == fathomable_core::annotations::Status::Resolved
            }),
        Action::RestoreThread => app
            .thread_cursor()
            .thread()
            .and_then(|id| app.thread(id))
            .is_some_and(fathomable_core::annotations::Thread::is_archived),
        Action::ArchiveResolved => app.store.as_ref().is_some_and(|store| {
            store
                .threads()
                .iter()
                .any(|thread| thread.status() == fathomable_core::annotations::Status::Resolved)
        }),
        Action::ClearBoard => app
            .store
            .as_ref()
            .is_some_and(|store| !store.threads().is_empty()),
        Action::SidebarToggle => app.sidebar.shown() || app.sidebar.has_restore(),
        _ => true,
    }
}

fn action_checked(app: &App, action: Action) -> bool {
    match action {
        Action::SidebarToggle => app.sidebar.shown(),
        Action::TreeToggle => app.sidebar.tree,
        Action::ThreadsPaneToggle => app.sidebar.threads,
        Action::ComparisonWhitespace => {
            app.comparison.compare().whitespace == fathomable_core::diff::Whitespace::Ignore
        }
        _ => false,
    }
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
            Target::Licenses => {
                self.open_licenses();
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
    key: Key::Char('s'),
    ctrl: false,
    alt: false,
}];
const STATUS_KEYS: [Chord; 7] = colon("status");
const HELP_KEYS: [Chord; 5] = colon("help");
const DOCTOR_KEYS: [Chord; 7] = colon("doctor");
const LICENSES_KEYS: [Chord; 9] = colon("licenses");
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
        Target::ReviewResolved => Some(&REVIEW_RESOLVED_KEY),
        Target::ReviewFile => Some(&REVIEW_FILE_KEY),
        Target::GettingStarted => Some(&HELP_KEYS),
        Target::Doctor => Some(&DOCTOR_KEYS),
        Target::Licenses => Some(&LICENSES_KEYS),
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

fn bar_mouse(app: &mut App, kind: MouseEventKind, column: usize) -> Effect {
    let left = kind == MouseEventKind::Down(MouseButton::Left);
    if let Some(root) = label_at(app.width, column) {
        match kind {
            MouseEventKind::Moved if app.menu_bar.open().is_some_and(|open| open.root != root) => {
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
    } else if let Some(picker) = bar_tail(app, app.width).picker_at(column) {
        if left {
            app.close_title_menu();
            app.open_picker(picker);
        }
    } else if bar_identity(app, app.width)
        .and_then(|identity| identity.picker_at(column))
        .is_some()
    {
        if left {
            app.close_title_menu();
            app.pick_worktree();
        }
    } else if left {
        app.close_title_menu();
    }
    Effect::None
}

/// The title bar and its open menu's share of one mouse event. `None`
/// lets an event outside a closed menu continue to the panes.
pub(crate) fn mouse(app: &mut App, event: MouseEvent) -> Option<Effect> {
    if !app.menu_bar.shown() {
        return None;
    }
    let column = usize::from(event.column);
    let row = usize::from(event.row);
    if row == 0 {
        return Some(bar_mouse(app, event.kind, column));
    }
    let left = event.kind == MouseEventKind::Down(MouseButton::Left);
    let right = event.kind == MouseEventKind::Down(MouseButton::Right);
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
    use fathomable_core::config::{DiffMode, SidebarConfig};
    use fathomable_core::layout::display_width;
    use fathomable_core::theme::Theme as CoreTheme;
    use ratatui::style::{Color, Modifier};

    use super::{
        Focused, Root, Submenu, bar_identity, child_layout, identity_within, labels, root_layout,
        rows, submenu_rows,
    };
    use crate::app::input::{keys, mouse};
    use crate::app::testing;

    #[test]
    fn labels_collapse_to_the_application_menu_when_narrow() {
        assert_eq!(labels(80).len(), 5);
        assert_eq!(labels(20).len(), 1);
    }

    #[test]
    fn menus_are_stable() -> anyhow::Result<()> {
        let dir = testing::workspace("menu-bar-model", testing::README)?;
        let app = testing::app(&dir)?;
        assert_eq!(rows(&app, Root::App).len(), 5);
        assert_eq!(rows(&app, Root::Layout).len(), 6);
        assert_eq!(rows(&app, Root::Go).len(), 6);
        assert_eq!(rows(&app, Root::Review).len(), 16);
        let diff_rows = rows(&app, Root::Diff);
        assert_eq!(diff_rows.len(), 11);
        let diff_labels = diff_rows
            .iter()
            .filter_map(super::Row::item)
            .map(|item| item.label.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            diff_labels,
            [
                "Standard diff",
                "Unified diff",
                "Diff off",
                "Pick base…",
                "Pick target…",
                "Head to WorkingTree",
                "Save review point",
                "Ignore whitespace",
            ]
        );
        let review = rows(&app, Root::Review);
        assert_eq!(
            review[0].item().map(|item| (&*item.label, item.checked)),
            Some(("Reviews", false))
        );
        assert_eq!(
            review[5].item().map(|item| (&*item.label, item.checked)),
            Some(("Show resolved", false))
        );
        assert_eq!(
            review[6].item().map(|item| (&*item.label, item.checked)),
            Some(("Only current file", false))
        );
        assert_eq!(submenu_rows(&app, super::Submenu::Help).len(), 4);
        let go = rows(&app, Root::Go);
        assert_eq!(go[0].item().map(|item| item.hint.as_str()), Some("Sp f"));
        assert_eq!(go[1].item().map(|item| item.hint.as_str()), Some("Sp F i"));
        assert!(
            go.iter()
                .filter_map(super::Row::item)
                .all(|item| item.label != "Newest change")
        );
        assert!(
            !rows(&app, Root::App)
                .iter()
                .filter_map(super::Row::item)
                .any(|item| item.target == super::Target::Action(super::Action::MenuBarToggle)),
            "the menu bar can only be hidden through Space p m"
        );
        Ok(())
    }

    #[test]
    fn source_action_follows_file_eligibility_and_unified_mode() -> anyhow::Result<()> {
        let dir = testing::workspace("menu-source-diff-mode", testing::README)?;
        std::fs::write(dir.0.join("ws/main.rs"), "fn main() {}\n")?;
        let mut app = testing::AppBuilder::new(&dir).unopened().build()?;
        assert!(!super::action_available(&app, super::Action::SourceView));
        app.open(std::path::Path::new("README.md"));
        assert!(super::action_available(&app, super::Action::SourceView));
        app.open(std::path::Path::new("main.rs"));
        assert!(!super::action_available(&app, super::Action::SourceView));
        app.open(std::path::Path::new("README.md"));
        app.select_diff_mode(fathomable_core::config::DiffMode::Unified);
        assert!(!super::action_available(&app, super::Action::SourceView));
        Ok(())
    }

    #[test]
    fn diff_menu_choices_and_dormant_whitespace_are_exact() -> anyhow::Result<()> {
        let mut app = shown_app("menu-diff-choices")?;
        app.toggle_whitespace();
        app.select_diff_mode(DiffMode::Off);

        let rows = rows(&app, Root::Diff);
        let items = rows.iter().filter_map(super::Row::item).collect::<Vec<_>>();
        assert_eq!(
            items
                .iter()
                .map(|item| (item.label.as_str(), item.active))
                .collect::<Vec<_>>(),
            [
                ("Standard diff", false),
                ("Unified diff", false),
                ("Diff off", true),
                ("Pick base…", false),
                ("Pick target…", false),
                ("Head to WorkingTree", false),
                ("Save review point", false),
                ("Ignore whitespace", false),
            ]
        );
        assert!(items[3].enabled, "Base is the intentional restore route");
        assert!(items[7].checked, "the whitespace preference is retained");
        assert!(!items[7].enabled, "but cannot run while Off");
        app.open_title_menu(Root::Diff);
        let screen = testing::screen(&app)?.join("\n");
        assert!(screen.contains("▌ Diff off"), "{screen}");
        assert!(screen.contains("Standard diff"), "{screen}");
        assert!(screen.contains("Unified diff"), "{screen}");
        assert!(screen.contains("Sp d s"), "{screen}");
        assert!(screen.contains("Sp d u"), "{screen}");
        assert!(screen.contains("Sp d o"), "{screen}");
        assert!(screen.contains("Sp d d"), "{screen}");
        assert!(!screen.contains("Comparison controls"), "{screen}");
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
    fn menu_bar_reserves_a_row_and_ends_with_the_comparison() -> anyhow::Result<()> {
        let app = shown_app("menu-bar-draw")?;
        assert_eq!(app.pane_top(), 1);
        assert_eq!(app.pane_rows(), 28);
        let screen = testing::screen(&app)?;
        assert!(screen[0].contains("☰") && screen[0].contains("Layout  Go  Review  Diff"));
        assert!(
            screen[0].trim_end().ends_with("EmptyTree to WorkingTree"),
            "{:?}",
            screen[0]
        );
        assert!(screen[0].contains("ws"), "{:?}", screen[0]);
        assert!(!screen[0].contains("README.md"), "{:?}", screen[0]);
        let identity = bar_identity(&app, app.size().0).context("bar identity")?;
        assert_eq!(identity.repo, "ws");
        assert_eq!(identity.x, (app.size().0 - display_width("ws")) / 2);
        let buffer = testing::buffer(&app)?;
        assert_eq!(buffer[(u16::try_from(identity.x)?, 0)].symbol(), "w");
        assert_eq!(
            buffer[(u16::try_from(identity.x + display_width("ws") - 1)?, 0)].symbol(),
            "s"
        );
        assert!(
            buffer[(u16::try_from(identity.x)?, 0)]
                .modifier
                .contains(Modifier::DIM),
            "the repository identity is dim with one worktree"
        );
        assert!(!screen[0].contains("SOURCE"), "{:?}", screen[0]);
        Ok(())
    }

    #[test]
    fn off_menu_bar_renders_only_the_clickable_target() -> anyhow::Result<()> {
        let mut app = shown_app("menu-bar-off-target")?;
        app.select_diff_mode(DiffMode::Off);
        let tail = super::bar_tail(&app, app.size().0);
        assert!(tail.base.is_none());
        let target = tail.target.context("Target should fit")?;
        assert_eq!(target.text, "WorkingTree");
        assert_eq!(target.picker, crate::app::PickerKind::ComparisonTarget);

        let screen = testing::screen(&app)?;
        assert!(
            screen[0].trim_end().ends_with("WorkingTree"),
            "{:?}",
            screen[0]
        );
        assert!(!screen[0].contains(" to "), "{:?}", screen[0]);
        assert!(!screen[0].contains("EmptyTree"), "{:?}", screen[0]);
        Ok(())
    }

    #[test]
    fn repository_identity_stays_centred_without_the_filename() -> anyhow::Result<()> {
        let app = shown_app("menu-bar-identity")?;
        let identity = identity_within(&app, 40, 12, 24).context("visible identity")?;
        assert_eq!(identity.x, (40 - identity.width) / 2);
        assert!(identity.x >= 12 && identity.x + identity.width <= 24);
        assert_eq!(identity.repo, "ws");
        assert!(identity_within(&app, 20, 10, 11).is_none());
        Ok(())
    }

    #[test]
    fn mouse_and_keys_walk_root_labels_and_an_aligned_submenu() -> anyhow::Result<()> {
        let mut app = shown_app("menu-bar-navigation")?;
        testing::click(&mut app, 12, 0);
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
        let child_rows = submenu_rows(&app, Submenu::Help);
        let child = child_layout(&app, root, &root_rows, &child_rows).context("help submenu")?;
        assert_eq!(child.y, root.y + 1, "submenu aligns with Help row");
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
    fn layout_menu_uses_stable_checked_labels() -> anyhow::Result<()> {
        let mut app = shown_app("layout-checks")?;
        let items = |app: &crate::app::App| {
            submenu_rows(app, Submenu::Layout)
                .into_iter()
                .filter_map(|row| row.item().cloned())
                .map(|item| (item.label, item.checked, item.active))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            items(&app),
            [
                ("File view".to_owned(), false, true),
                ("Reviews view".to_owned(), false, false),
                ("Sidebar".to_owned(), true, false),
                ("Files pane".to_owned(), true, false),
                ("Threads pane".to_owned(), true, false),
            ]
        );
        testing::click(&mut app, 4, 0);
        assert!(
            testing::screen(&app)?
                .iter()
                .any(|row| row.contains("▌ File view"))
        );
        app.close_title_menu();
        app.open_review();
        let review_items = items(&app);
        assert!(!review_items[0].2);
        assert!(review_items[1].2);
        app.open_file_view();
        testing::press(&mut app, " pf");
        assert_eq!(
            items(&app),
            [
                ("File view".to_owned(), false, true),
                ("Reviews view".to_owned(), false, false),
                ("Sidebar".to_owned(), true, false),
                ("Files pane".to_owned(), false, false),
                ("Threads pane".to_owned(), true, false),
            ]
        );
        testing::press(&mut app, " ps");
        assert_eq!(
            items(&app),
            [
                ("File view".to_owned(), false, true),
                ("Reviews view".to_owned(), false, false),
                ("Sidebar".to_owned(), false, false),
                ("Files pane".to_owned(), false, false),
                ("Threads pane".to_owned(), false, false),
            ]
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
        testing::click(&mut app, 12, 0);
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
                row: 2,
                modifiers: KeyModifiers::NONE,
            },
        );
        let root_rows = rows(&app, Root::App);
        let root = root_layout(&app, &root_rows).context("root menu")?;
        let child_rows = submenu_rows(&app, Submenu::Help);
        let child = child_layout(&app, root, &root_rows, &child_rows).context("help submenu")?;
        assert!(child.visible_rows >= 1);
        assert!(
            child.y > root.y,
            "the child border stays below the parent title"
        );

        app.close_title_menu();
        testing::click(&mut app, 1, 0);
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
        app.resize(20, 8);
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
            Some(Focused::Child(index)) if index >= 1
        ));
        Ok(())
    }

    #[test]
    fn menu_width_preserves_full_shortcuts_and_colon_commands_run() -> anyhow::Result<()> {
        let mut app = shown_app("menu-width")?;
        testing::click(&mut app, 12, 0);
        let go_rows = rows(&app, Root::Go);
        let layout = root_layout(&app, &go_rows).context("Go menu layout")?;
        let item = go_rows[1].item().context("ignored-files item")?;
        let row = layout.y + 2;
        let label_x = layout.x + 3;
        let hint_x = layout.x + layout.width - 1 - display_width(&item.hint);
        let buffer = testing::buffer(&app)?;
        let theme =
            crate::app::draw::Theme::from_core(&CoreTheme::resolve("default-dark", |_| Ok(None))?);
        assert_eq!(
            buffer[(u16::try_from(label_x)?, u16::try_from(row)?)].symbol(),
            "O"
        );
        assert_eq!(
            buffer[(u16::try_from(label_x)?, u16::try_from(row)?)].fg,
            theme.menu.fg.unwrap_or(Color::Reset),
            "the action uses the normal menu colour"
        );
        assert_eq!(
            buffer[(u16::try_from(hint_x)?, u16::try_from(row)?)].symbol(),
            "S"
        );
        assert_eq!(
            buffer[(u16::try_from(hint_x)?, u16::try_from(row)?)].fg,
            theme.info.fg.unwrap_or(Color::Reset),
            "the shortcut uses the subdued info colour"
        );
        assert_eq!(
            buffer[(u16::try_from(hint_x - 1)?, u16::try_from(row)?)].symbol(),
            " ",
            "the widest action and shortcut retain a gap"
        );
        assert_eq!(
            buffer[(
                u16::try_from(layout.x + layout.width - 2)?,
                u16::try_from(row)?,
            )]
                .symbol(),
            "i",
            "the shortcut is right-aligned"
        );
        assert!(
            testing::screen(&app)?
                .iter()
                .any(|row| row.contains("Sp F i")),
            "the longest Go hint is not clipped"
        );
        app.close_title_menu();
        testing::click(&mut app, 1, 0);
        testing::press(&mut app, ":about");
        assert!(matches!(app.popup(), Some(crate::app::Popup::About)));
        Ok(())
    }

    #[test]
    fn review_menu_focuses_an_open_review() -> anyhow::Result<()> {
        let mut app = shown_app("review-focus")?;
        app.open_review();
        app.window_files();
        assert!(app.review_list().is_open());
        assert_eq!(
            rows(&app, Root::Review)[0].item().map(|item| item.checked),
            Some(false),
            "Reviews is a command, not a checked toggle"
        );
        app.run_title_target(super::Target::Action(super::Action::Review));
        assert!(app.review_list().is_open());
        assert_eq!(app.focus(), crate::app::Focus::Review);
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
