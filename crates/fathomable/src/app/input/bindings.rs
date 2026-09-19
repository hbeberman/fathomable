// @okf-doc: /decisions/0045-bindings-are-data.md
//! The key bindings as one table: what each key does where, and the
//! words the help popup, the menus, and the hint bars show for it.
//!
//! A [`Binding`] pairs one [`Action`] with the key sequences that fire it
//! in one [`Where`]. Dispatch looks a typed sequence up with [`lookup`];
//! a sequence that is the start of a longer binding is a prefix and the
//! viewer waits for the rest, showing [`menu_sections`] meanwhile. The help
//! popup groups and wraps [`BINDINGS`], and a pane header asks
//! [`hint`] how a key is spelled, so no surface can name a key the table
//! does not bind.

use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A key on its own, without modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Char(' ') => f.write_str("Space"),
            Self::Char(ch) => write!(f, "{ch}"),
            Self::Enter => f.write_str("Enter"),
            Self::Esc => f.write_str("Esc"),
            Self::Tab => f.write_str("Tab"),
            Self::BackTab => f.write_str("Shift-Tab"),
            Self::Backspace => f.write_str("Backspace"),
            Self::Delete => f.write_str("Delete"),
            Self::Up => f.write_str("Up"),
            Self::Down => f.write_str("Down"),
            Self::Left => f.write_str("Left"),
            Self::Right => f.write_str("Right"),
            Self::Home => f.write_str("Home"),
            Self::End => f.write_str("End"),
        }
    }
}

/// One key press: a [`Key`] with its modifiers. Shift is retained for
/// Enter and arrow keys; shifted letters arrive as uppercase characters,
/// and Shift-Tab is normalized to [`Key::BackTab`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Chord {
    pub(crate) key: Key,
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
}

impl Chord {
    /// The chord a terminal event is, or `None` for a key the viewer
    /// never binds (function keys, media keys).
    #[must_use]
    pub(crate) fn from_event(event: KeyEvent) -> Option<Self> {
        let key = match event.code {
            KeyCode::Char(ch) => Key::Char(ch),
            KeyCode::Enter => Key::Enter,
            KeyCode::Esc => Key::Esc,
            KeyCode::Tab if event.modifiers.contains(KeyModifiers::SHIFT) => Key::BackTab,
            KeyCode::Tab => Key::Tab,
            KeyCode::BackTab => Key::BackTab,
            KeyCode::Backspace => Key::Backspace,
            KeyCode::Delete => Key::Delete,
            KeyCode::Up => Key::Up,
            KeyCode::Down => Key::Down,
            KeyCode::Left => Key::Left,
            KeyCode::Right => Key::Right,
            KeyCode::Home => Key::Home,
            KeyCode::End => Key::End,
            _ => return None,
        };
        Some(Self {
            key,
            ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
            alt: event.modifiers.contains(KeyModifiers::ALT),
            shift: event.modifiers.contains(KeyModifiers::SHIFT)
                && matches!(
                    key,
                    Key::Enter | Key::Up | Key::Down | Key::Left | Key::Right
                ),
        })
    }

    /// Whether this is a bare character: no modifier, not the space bar.
    #[must_use]
    pub(crate) fn is_plain_char(self) -> bool {
        !self.ctrl && !self.alt && !self.shift && matches!(self.key, Key::Char(ch) if ch != ' ')
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl-")?;
        }
        if self.alt {
            f.write_str("Alt-")?;
        }
        if self.shift {
            f.write_str("Shift-")?;
        }
        write!(f, "{}", self.key)
    }
}

/// A plain character key.
const fn c(ch: char) -> Chord {
    Chord {
        key: Key::Char(ch),
        ctrl: false,
        alt: false,
        shift: false,
    }
}

/// A key with Ctrl held.
const fn ctrl(ch: char) -> Chord {
    Chord {
        key: Key::Char(ch),
        ctrl: true,
        alt: false,
        shift: false,
    }
}

/// A key with Alt held.
const fn alt(key: Key) -> Chord {
    Chord {
        key,
        ctrl: false,
        alt: true,
        shift: false,
    }
}

/// A named key with Shift held.
const fn shift(key: Key) -> Chord {
    Chord {
        key,
        ctrl: false,
        alt: false,
        shift: true,
    }
}

/// A named key without modifiers.
const fn k(key: Key) -> Chord {
    Chord {
        key,
        ctrl: false,
        alt: false,
        shift: false,
    }
}

/// A key sequence: one chord, or a prefix and what follows it.
pub(crate) type Keys = &'static [Chord];

/// How a sequence is written: bare characters run together (`gg`, `]w`),
/// anything else is space-separated (`Space d s`, `Ctrl-d`).
#[must_use]
pub(crate) fn spell(keys: &[Chord]) -> String {
    spell_with_space(keys, "Space")
}

/// Spell a sequence compactly for action-menu shortcut columns.
#[must_use]
pub(crate) fn menu_spell(keys: &[Chord]) -> String {
    spell_with_space(keys, "Sp")
}

fn spell_with_space(keys: &[Chord], space: &str) -> String {
    let separator = if keys.iter().all(|chord| chord.is_plain_char()) {
        ""
    } else {
        " "
    };
    keys.iter()
        .map(|chord| {
            let key = if chord.key == Key::Char(' ') {
                space.to_owned()
            } else {
                chord.key.to_string()
            };
            format!(
                "{}{}{}{key}",
                if chord.ctrl { "Ctrl-" } else { "" },
                if chord.alt { "Alt-" } else { "" },
                if chord.shift { "Shift-" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(separator)
}

/// Which surface a binding lives on: the pane that has focus, the popup
/// that is open, or [`Where::Any`] for every pane at once. `Any` never
/// reaches a popup: the draft, the picker, and the command line
/// take only their own keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Where {
    /// The text, in normal or select mode.
    View,
    /// The file tree.
    Tree,
    /// The sidebar's threads pane.
    ThreadsPane,
    /// The review list.
    Review,
    /// The draft being written in the text (ADR 0054).
    Draft,
    /// A file, diff, or worktree picker.
    Picker,
    /// The `:` and `/` input line.
    Input,
    /// Every pane: the view, the tree, and the two thread surfaces.
    Any,
}

impl Where {
    /// Whether `Any` bindings apply here: in a pane, not a popup.
    #[must_use]
    pub(crate) fn takes_any(self) -> bool {
        !matches!(self, Self::Draft | Self::Picker | Self::Input)
    }
}

macro_rules! actions {
    ($($(#[$meta:meta])* $name:ident),* $(,)?) => {
        /// What a key does. One action can be bound on several surfaces;
        /// `App::act` gives it that surface's meaning.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum Action {
            $($(#[$meta])* $name),*
        }

        impl Action {
            /// Every action, for the test that each one is bound.
            #[cfg(test)]
            pub(crate) const ALL: &'static [Self] = &[$(Self::$name),*];
        }
    };
}

actions! {
    MoveDown,
    MoveUp,
    MoveLeft,
    MoveRight,
    LineStart,
    LineEnd,
    GotoLineStart,
    GotoLineEnd,
    Top,
    Bottom,
    HalfPageDown,
    HalfPageUp,
    ScrollDown,
    ScrollUp,
    SearchForward,
    SearchBackward,
    SearchNext,
    SearchPrev,
    SelectChars,
    SelectLines,
    ExtendLine,
    Yank,
    GotoFile,
    CopyPath,
    CopyFullPath,
    Comment,
    NewThread,
    /// `Space t f`: a comment on the open file as a whole (ADR 0063).
    FileComment,
    SourceView,
    DiffStandard,
    DiffUnified,
    DiffOff,
    ComparisonSave,
    ComparisonManage,
    ComparisonBase,
    ComparisonTarget,
    ComparisonHeadWorkingTree,
    /// `Space d l`: compare the first parent of `HEAD` to `HEAD`.
    ComparisonHeadParent,
    /// `Space d c`: choose a commit and compare its first parent to it.
    ComparisonCommitParent,
    /// `]w`: the next worktree (ADR 0070).
    WorktreeNext,
    /// `[w`: the previous worktree (ADR 0070).
    WorktreePrev,
    ComparisonWhitespace,
    StubsToggle,
    /// `Space F c`: only changed files in the files pane (ADR 0068).
    FilesChanged,
    /// `Space F o`: only files with reviews in the files pane (ADR 0068).
    FilesReviews,
    /// `Space F u`: hide untracked files in the files pane (ADR 0068).
    FilesUntracked,
    /// `Space F i`: show ignored files in the files pane (ADR 0068).
    FilesIgnored,
    ChangeNext,
    ChangePrev,
    ChangeFileNext,
    ChangeFilePrev,
    JumpBack,
    JumpForward,
    /// `q`: open the guarded quit confirmation from any normal pane.
    ConfirmQuit,
    CommandLine,
    Escape,
    Confirm,
    TreeToggle,
    PickFile,
    PickAnyFile,
    PickRecent,
    /// `f`: open and focus the file view.
    FileView,
    Review,
    ReviewRecentlyResolved,
    ReviewArchived,
    ArchiveResolved,
    ClearBoard,
    ArchiveThread,
    RestoreThread,
    /// `Space p s`: hide or show the sidebar as one remembered layout.
    SidebarToggle,
    /// `Space p m`: hide or show the persistent menu bar.
    MenuBarToggle,
    ThreadsPaneToggle,
    WindowNext,
    WindowPrev,
    WindowFiles,
    WindowThreads,
    ApplicationMenu,
    PaneScope,
    ReviewResolved,
    Help,
    ThreadNext,
    ThreadPrev,
    OpenThreadNext,
    OpenThreadPrev,
    Reply,
    EditMessage,
    EditNewestOwn,
    StubResolvedToggle,
    ToggleResolved,
    ToggleAutoResolve,
    Delete,
    DeleteThread,
    Fold,
    FoldAll,
    FileOnly,
    Newline,
    SubmitAutoResolve,
    CompleteNext,
    CompletePrevious,
    Backspace,
    DeleteForward,
    WordBack,
    WordForward,
    DeleteWordBack,
    DeleteToLineStart,
    DeleteToLineEnd,
    ClearDraft,
    EditDraft,
}

/// One row of the table: the sequences that fire `action` in `place`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Binding {
    pub(crate) keys: &'static [Keys],
    pub(crate) place: Where,
    pub(crate) action: Action,
    pub(crate) label: &'static str,
    pub(crate) group: &'static str,
    menu_section: u8,
}

const fn bind(
    place: Where,
    keys: &'static [Keys],
    action: Action,
    group: &'static str,
    label: &'static str,
) -> Binding {
    Binding {
        keys,
        place,
        action,
        label,
        group,
        menu_section: 0,
    }
}

const fn bind_in(
    place: Where,
    keys: &'static [Keys],
    action: Action,
    group: &'static str,
    label: &'static str,
    menu_section: u8,
) -> Binding {
    Binding {
        keys,
        place,
        action,
        label,
        group,
        menu_section,
    }
}

use Action as A;
use Key as K;
use Where as W;

/// Every binding. Help keeps this order within each group and for groups
/// that do not have a preferred first-screen lane.
pub(crate) const BINDINGS: &[Binding] = &[
    // ----- the text -----
    bind(
        W::View,
        &[&[c('j')], &[k(K::Down)]],
        A::MoveDown,
        "Move",
        "down",
    ),
    bind(W::View, &[&[c('k')], &[k(K::Up)]], A::MoveUp, "Move", "up"),
    bind(
        W::View,
        &[&[c('h')], &[k(K::Left)]],
        A::MoveLeft,
        "Move",
        "left, wraps to row above",
    ),
    bind(
        W::View,
        &[&[c('l')], &[k(K::Right)]],
        A::MoveRight,
        "Move",
        "right, wraps to row below",
    ),
    bind(
        W::View,
        &[&[c('0')], &[k(K::Home)]],
        A::LineStart,
        "Move",
        "line start",
    ),
    bind(
        W::View,
        &[&[c('$')], &[k(K::End)]],
        A::LineEnd,
        "Move",
        "line end",
    ),
    bind(W::View, &[&[c('g'), c('g')]], A::Top, "Move", "goto top"),
    bind(
        W::View,
        &[&[c('g'), c('e')], &[c('G')]],
        A::Bottom,
        "Move",
        "goto bottom",
    ),
    bind(
        W::View,
        &[&[c('g'), c('l')]],
        A::GotoLineEnd,
        "Move",
        "goto line end",
    ),
    bind(
        W::View,
        &[&[c('g'), c('h')]],
        A::GotoLineStart,
        "Move",
        "goto line start",
    ),
    bind(
        W::View,
        &[&[ctrl('d')]],
        A::HalfPageDown,
        "Move",
        "half page down",
    ),
    bind(
        W::View,
        &[&[ctrl('u')]],
        A::HalfPageUp,
        "Move",
        "half page up",
    ),
    bind(
        W::View,
        &[&[c('/')]],
        A::SearchForward,
        "Search",
        "search forward",
    ),
    bind(
        W::View,
        &[&[c('?')]],
        A::SearchBackward,
        "Search",
        "search backward",
    ),
    bind(W::View, &[&[c('n')]], A::SearchNext, "Search", "next match"),
    bind(
        W::View,
        &[&[c('N')]],
        A::SearchPrev,
        "Search",
        "previous match",
    ),
    bind(
        W::View,
        &[&[c('v')]],
        A::SelectChars,
        "Select",
        "select text",
    ),
    bind(
        W::View,
        &[&[c('V')]],
        A::SelectLines,
        "Select",
        "select lines",
    ),
    bind(
        W::View,
        &[&[c('x')]],
        A::ExtendLine,
        "Select",
        "select the line; again, one more below",
    ),
    bind(
        W::View,
        &[&[c('y')]],
        A::Yank,
        "Select",
        "copy the selection, or the line",
    ),
    bind(
        W::View,
        &[&[c('g'), c('f')]],
        A::GotoFile,
        "Links",
        "open linked file/URL",
    ),
    bind(W::View, &[&[c('c')]], A::Comment, "Threads", "comment"),
    bind(
        W::View,
        &[&[k(K::Enter)]],
        A::Confirm,
        "Threads",
        "fold or expand thread header",
    ),
    bind(
        W::View,
        &[&[c('z')]],
        A::Fold,
        "Threads",
        "expand or fold thread",
    ),
    bind(
        W::View,
        &[&[c('Z')]],
        A::FoldAll,
        "Threads",
        "expand or fold all",
    ),
    bind(
        W::View,
        &[&[c('e')]],
        A::EditMessage,
        "Threads",
        "edit your message",
    ),
    bind(
        W::View,
        &[&[c('r')]],
        A::ToggleResolved,
        "Threads",
        "resolve or reopen",
    ),
    bind(
        W::View,
        &[&[c('R')]],
        A::ToggleAutoResolve,
        "Threads",
        "toggle auto-resolve",
    ),
    bind(
        W::View,
        &[&[c('a')]],
        A::ArchiveThread,
        "Threads",
        "archive thread",
    ),
    bind(
        W::View,
        &[&[c('d'), c('d')]],
        A::Delete,
        "Threads",
        "delete thread",
    ),
    bind(
        W::Any,
        &[&[k(K::Tab)]],
        A::OpenThreadNext,
        "Navigation",
        "next open review thread",
    ),
    bind(
        W::Any,
        &[&[k(K::BackTab)]],
        A::OpenThreadPrev,
        "Navigation",
        "previous open review thread",
    ),
    bind(W::Any, &[&[c('w')]], A::WindowNext, "Focus", "next pane"),
    bind(
        W::Any,
        &[&[c('W')]],
        A::WindowPrev,
        "Focus",
        "previous pane",
    ),
    bind(W::Any, &[&[c('F')]], A::WindowFiles, "Focus", "File list"),
    bind(
        W::Any,
        &[&[c('T')]],
        A::WindowThreads,
        "Focus",
        "Thread list",
    ),
    bind(
        W::Any,
        &[&[alt(K::Char(' '))]],
        A::ApplicationMenu,
        "Focus",
        "application menu",
    ),
    bind(
        W::Any,
        &[&[c('J')], &[shift(K::Down)]],
        A::ChangeNext,
        "Navigation",
        "next comparison change",
    ),
    bind(
        W::Any,
        &[&[c('K')], &[shift(K::Up)]],
        A::ChangePrev,
        "Navigation",
        "previous comparison change",
    ),
    bind(
        W::Any,
        &[&[c('L')], &[shift(K::Right)]],
        A::ChangeFileNext,
        "Navigation",
        "next changed file",
    ),
    bind(
        W::Any,
        &[&[c('H')], &[shift(K::Left)]],
        A::ChangeFilePrev,
        "Navigation",
        "previous changed file",
    ),
    bind(
        W::Any,
        &[&[alt(K::Left)]],
        A::JumpBack,
        "Jumplist",
        "back to the position the last far move left",
    ),
    bind(
        W::Any,
        &[&[alt(K::Right)]],
        A::JumpForward,
        "Jumplist",
        "forward again",
    ),
    bind(
        W::Any,
        &[&[c('q')]],
        A::ConfirmQuit,
        "Commands",
        "quit with confirmation",
    ),
    bind(
        W::View,
        &[&[k(K::Esc)]],
        A::Escape,
        "Commands",
        "clear the input, prefix, selection, or highlight",
    ),
    bind(
        W::Any,
        &[&[c(':')]],
        A::CommandLine,
        "Commands",
        "command line",
    ),
    // ----- the Space menu (ADR 0056) -----
    bind(
        W::Any,
        &[&[c(' '), c('f'), c('f')]],
        A::PickFile,
        "Space menu",
        "open files: open file",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('f'), c('i')]],
        A::PickAnyFile,
        "Space menu",
        "open files: open file incl. ignored",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('f'), c('r')]],
        A::PickRecent,
        "Space menu",
        "open files: recent files",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('F'), c('c')]],
        A::FilesChanged,
        "Space menu",
        "files: only changed",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('F'), c('o')]],
        A::FilesReviews,
        "Space menu",
        "files: only reviews",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('F'), c('u')]],
        A::FilesUntracked,
        "Space menu",
        "files: hide untracked",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('F'), c('i')]],
        A::FilesIgnored,
        "Space menu",
        "files: show ignored",
    ),
    bind(W::Any, &[&[c('f')]], A::FileView, "Views", "open File"),
    bind(W::Any, &[&[c('t')]], A::Review, "Threads", "open Threads"),
    bind(
        W::Any,
        &[&[c(' '), c('t'), c('R')]],
        A::ReviewRecentlyResolved,
        "Space menu",
        "threads: recently resolved",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('t'), c('h')]],
        A::ReviewArchived,
        "Space menu",
        "threads: archived threads",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('t'), c('a')]],
        A::ArchiveResolved,
        "Space menu",
        "threads: archive resolved",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('t'), c('A')]],
        A::ClearBoard,
        "Space menu",
        "threads: clear board",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('p'), c('f')]],
        A::TreeToggle,
        "Space menu",
        "panes: toggle File list",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('p'), c('s')]],
        A::SidebarToggle,
        "Space menu",
        "panes: toggle sidebar",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('p'), c('m')]],
        A::MenuBarToggle,
        "Space menu",
        "panes: toggle menu bar",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('p'), c('t')]],
        A::ThreadsPaneToggle,
        "Space menu",
        "panes: toggle Thread list",
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('t'), c('c')]],
        A::NewThread,
        "Space menu",
        "threads: new thread",
        1,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('t'), c('r')]],
        A::Reply,
        "Space menu",
        "threads: reply",
        1,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('t'), c('e')]],
        A::EditNewestOwn,
        "Space menu",
        "threads: edit message",
        1,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('t'), c('d')]],
        A::DeleteThread,
        "Space menu",
        "threads: delete thread",
        1,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('t'), c('f')]],
        A::FileComment,
        "Space menu",
        "threads: file comment",
        1,
    ),
    bind(
        W::Any,
        &[&[c(' '), c('v'), c('s')]],
        A::SourceView,
        "Space menu",
        "view: source view",
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('v'), c('t')]],
        A::StubsToggle,
        "Space menu",
        "view: toggle thread stubs",
        1,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('v'), c('r')]],
        A::StubResolvedToggle,
        "Space menu",
        "view: toggle resolved stubs",
        1,
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('s')]],
        A::DiffStandard,
        "Space menu",
        "diff: standard",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('u')]],
        A::DiffUnified,
        "Space menu",
        "diff: unified",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('o')]],
        A::DiffOff,
        "Space menu",
        "diff: off",
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('d'), c('b')]],
        A::ComparisonBase,
        "Space menu",
        "diff: pick base…",
        1,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('d'), c('t')]],
        A::ComparisonTarget,
        "Space menu",
        "diff: pick target…",
        1,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('d'), c('d')]],
        A::ComparisonHeadWorkingTree,
        "Space menu",
        "diff: HEAD to Working tree",
        1,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('d'), c('l')]],
        A::ComparisonHeadParent,
        "Space menu",
        "diff: HEAD~1 to HEAD",
        1,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('d'), c('c')]],
        A::ComparisonCommitParent,
        "Space menu",
        "diff: Commit~1 to Commit…",
        1,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('d'), c('p')]],
        A::ComparisonSave,
        "Space menu",
        "diff: save review point",
        2,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('d'), c('r')]],
        A::ComparisonManage,
        "Space menu",
        "diff: manage review points…",
        2,
    ),
    bind_in(
        W::Any,
        &[&[c(' '), c('d'), c('w')]],
        A::ComparisonWhitespace,
        "Space menu",
        "diff: whitespace",
        3,
    ),
    bind(
        W::Any,
        &[&[c(']'), c('w')]],
        A::WorktreeNext,
        "Worktrees",
        "next worktree",
    ),
    bind(
        W::Any,
        &[&[c('['), c('w')]],
        A::WorktreePrev,
        "Worktrees",
        "previous worktree",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('?')]],
        A::Help,
        "Space menu",
        "view keymap",
    ),
    // ----- the tree -----
    bind(
        W::Tree,
        &[&[c('z')]],
        A::Fold,
        "File list",
        "fold or unfold nearest directory",
    ),
    bind(
        W::Tree,
        &[&[c('Z')]],
        A::FoldAll,
        "File list",
        "fold every directory, or unfold them all",
    ),
    bind(
        W::Tree,
        &[&[c('j')], &[k(K::Down)]],
        A::MoveDown,
        "File list",
        "down, showing the file",
    ),
    bind(
        W::Tree,
        &[&[c('k')], &[k(K::Up)]],
        A::MoveUp,
        "File list",
        "up, showing the file",
    ),
    bind(
        W::Tree,
        &[&[c('h')], &[k(K::Left)]],
        A::MoveLeft,
        "File list",
        "collapse",
    ),
    bind(
        W::Tree,
        &[&[c('l')], &[k(K::Right)]],
        A::MoveRight,
        "File list",
        "expand directory",
    ),
    bind(
        W::Tree,
        &[&[k(K::Enter)]],
        A::Confirm,
        "File list",
        "open and focus the text",
    ),
    bind(
        W::Tree,
        &[&[c('g'), c('g')]],
        A::Top,
        "File list",
        "goto top",
    ),
    bind(
        W::Tree,
        &[&[c('g'), c('e')], &[c('G')]],
        A::Bottom,
        "File list",
        "goto bottom",
    ),
    bind(
        W::Tree,
        &[&[c('y')]],
        A::CopyPath,
        "File list",
        "copy the path",
    ),
    bind(
        W::Tree,
        &[&[c('Y')]],
        A::CopyFullPath,
        "File list",
        "copy the full path",
    ),
    bind(
        W::Tree,
        &[&[c('c')]],
        A::FilesChanged,
        "File list",
        "only changed",
    ),
    bind(
        W::Tree,
        &[&[c('o')]],
        A::FilesReviews,
        "File list",
        "only reviews",
    ),
    bind(
        W::Tree,
        &[&[c('u')]],
        A::FilesUntracked,
        "File list",
        "hide untracked",
    ),
    bind(
        W::Tree,
        &[&[c('i')]],
        A::FilesIgnored,
        "File list",
        "show ignored",
    ),
    bind(
        W::Tree,
        &[&[k(K::Esc)]],
        A::Escape,
        "File list",
        "back to the text",
    ),
    // ----- the threads pane -----
    bind(
        W::ThreadsPane,
        &[&[c('j')], &[k(K::Down)]],
        A::MoveDown,
        "Thread list",
        "next thread; the text follows",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('k')], &[k(K::Up)]],
        A::MoveUp,
        "Thread list",
        "previous thread; the text follows",
    ),
    bind(
        W::ThreadsPane,
        &[&[k(K::Enter)]],
        A::Confirm,
        "Thread list",
        "open the file with the thread expanded",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('s')]],
        A::PaneScope,
        "Thread list",
        "this file, or the workspace",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('x')]],
        A::ReviewResolved,
        "Thread list",
        "show or hide resolved threads",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('T'), c('s')]],
        A::PaneScope,
        "Space menu",
        "thread list: this file, or the workspace",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('T'), c('x')]],
        A::ReviewResolved,
        "Space menu",
        "thread list: show or hide resolved threads",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('z')]],
        A::Fold,
        "Thread list",
        "fold or unfold the file",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('Z')]],
        A::FoldAll,
        "Thread list",
        "fold every file, or unfold them all",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('c')]],
        A::Reply,
        "Thread list",
        "reply",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('r')]],
        A::ToggleResolved,
        "Thread list",
        "resolve or reopen",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('R')]],
        A::ToggleAutoResolve,
        "Thread list",
        "toggle auto-resolve",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('a')]],
        A::ArchiveThread,
        "Thread list",
        "archive thread",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('u')]],
        A::RestoreThread,
        "Thread list",
        "restore thread",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('d'), c('d')]],
        A::Delete,
        "Thread list",
        "delete the thread",
    ),
    bind(
        W::ThreadsPane,
        &[&[k(K::Esc)]],
        A::Escape,
        "Thread list",
        "back to the text; the pane stays",
    ),
    // ----- the review list -----
    bind(
        W::Review,
        &[&[c('j')], &[k(K::Down)]],
        A::ThreadNext,
        "Threads",
        "next thread",
    ),
    bind(
        W::Review,
        &[&[c('k')], &[k(K::Up)]],
        A::ThreadPrev,
        "Threads",
        "previous thread",
    ),
    bind(
        W::Review,
        &[&[c('l')], &[k(K::Right)]],
        A::MoveDown,
        "Threads",
        "next message",
    ),
    bind(
        W::Review,
        &[&[c('h')], &[k(K::Left)]],
        A::MoveUp,
        "Threads",
        "previous message",
    ),
    bind(
        W::Review,
        &[&[c('g'), c('g')]],
        A::Top,
        "Threads",
        "goto top",
    ),
    bind(
        W::Review,
        &[&[c('g'), c('e')], &[c('G')]],
        A::Bottom,
        "Threads",
        "goto bottom",
    ),
    bind(
        W::Review,
        &[&[ctrl('d')]],
        A::HalfPageDown,
        "Threads",
        "half a page down",
    ),
    bind(
        W::Review,
        &[&[ctrl('u')]],
        A::HalfPageUp,
        "Threads",
        "half a page up",
    ),
    bind(
        W::Review,
        &[&[k(K::Enter)]],
        A::Confirm,
        "Threads",
        "open the file with the thread expanded on this message",
    ),
    bind(W::Review, &[&[c('c')]], A::Reply, "Threads", "reply"),
    bind(
        W::Review,
        &[&[c('a')]],
        A::ArchiveThread,
        "Threads",
        "archive thread",
    ),
    bind(
        W::Review,
        &[&[c('u')]],
        A::RestoreThread,
        "Threads",
        "restore thread",
    ),
    bind(
        W::Review,
        &[&[c('e')]],
        A::EditMessage,
        "Threads",
        "edit your message",
    ),
    bind(
        W::Review,
        &[&[c('r')]],
        A::ToggleResolved,
        "Threads",
        "resolve or reopen",
    ),
    bind(
        W::Review,
        &[&[c('R')]],
        A::ToggleAutoResolve,
        "Threads",
        "toggle auto-resolve",
    ),
    bind(
        W::Review,
        &[&[c('d'), c('d')]],
        A::Delete,
        "Threads",
        "delete the thread",
    ),
    bind(
        W::Review,
        &[&[c('z')]],
        A::Fold,
        "Threads",
        "fold or expand the thread here, or the file on its row",
    ),
    bind(
        W::Review,
        &[&[c('Z')]],
        A::FoldAll,
        "Threads",
        "fold every thread, or expand them all",
    ),
    bind(
        W::Review,
        &[&[c('x')]],
        A::ReviewResolved,
        "Threads",
        "show or hide resolved threads",
    ),
    bind(
        W::Review,
        &[&[c('s')]],
        A::FileOnly,
        "Threads",
        "only this file",
    ),
    bind(
        W::Review,
        &[&[k(K::Esc)]],
        A::Escape,
        "Threads",
        "back to the text",
    ),
    // ----- the draft -----
    bind(
        W::Draft,
        &[&[k(K::Enter)]],
        A::Confirm,
        "Draft",
        "submit, or save",
    ),
    bind(
        W::Draft,
        &[&[shift(K::Enter)], &[alt(K::Enter)]],
        A::Newline,
        "Draft",
        "newline",
    ),
    bind(
        W::Draft,
        &[&[Chord {
            key: K::Enter,
            ctrl: true,
            alt: false,
            shift: false,
        }]],
        A::SubmitAutoResolve,
        "Draft",
        "submit and enable auto-resolve",
    ),
    bind(
        W::Draft,
        &[&[alt(K::Char('k'))], &[alt(K::Up)]],
        A::ScrollUp,
        "Draft",
        "scroll the text up",
    ),
    bind(
        W::Draft,
        &[&[alt(K::Char('j'))], &[alt(K::Down)]],
        A::ScrollDown,
        "Draft",
        "scroll the text down",
    ),
    bind(
        W::Draft,
        &[&[ctrl('e')]],
        A::EditDraft,
        "Draft",
        "edit the draft in $EDITOR",
    ),
    bind(W::Draft, &[&[k(K::Left)]], A::MoveLeft, "Draft", "left"),
    bind(W::Draft, &[&[k(K::Right)]], A::MoveRight, "Draft", "right"),
    bind(W::Draft, &[&[k(K::Up)]], A::MoveUp, "Draft", "up"),
    bind(W::Draft, &[&[k(K::Down)]], A::MoveDown, "Draft", "down"),
    bind(
        W::Draft,
        &[&[k(K::Home)], &[ctrl('a')]],
        A::LineStart,
        "Draft",
        "line start",
    ),
    bind(W::Draft, &[&[k(K::End)]], A::LineEnd, "Draft", "line end"),
    bind(
        W::Draft,
        &[&[alt(K::Char('b'))]],
        A::WordBack,
        "Draft",
        "word back",
    ),
    bind(
        W::Draft,
        &[&[alt(K::Char('f'))]],
        A::WordForward,
        "Draft",
        "word forward",
    ),
    bind(
        W::Draft,
        &[&[k(K::Backspace)]],
        A::Backspace,
        "Draft",
        "delete back",
    ),
    bind(
        W::Draft,
        &[&[k(K::Delete)]],
        A::DeleteForward,
        "Draft",
        "delete forward",
    ),
    bind(
        W::Draft,
        &[&[ctrl('w')]],
        A::DeleteWordBack,
        "Draft",
        "delete the word back",
    ),
    bind(
        W::Draft,
        &[&[ctrl('u')]],
        A::DeleteToLineStart,
        "Draft",
        "delete to line start",
    ),
    bind(
        W::Draft,
        &[&[ctrl('k')]],
        A::DeleteToLineEnd,
        "Draft",
        "delete to line end",
    ),
    bind(
        W::Draft,
        &[&[ctrl('c')]],
        A::ClearDraft,
        "Draft",
        "clear the draft; empty closes",
    ),
    bind(
        W::Draft,
        &[&[k(K::Esc)]],
        A::Escape,
        "Draft",
        "cancel; twice after a change",
    ),
    // ----- the picker -----
    bind(
        W::Picker,
        &[&[k(K::Down)], &[ctrl('j')]],
        A::MoveDown,
        "Picker",
        "down",
    ),
    bind(
        W::Picker,
        &[&[k(K::Up)], &[ctrl('k')]],
        A::MoveUp,
        "Picker",
        "up",
    ),
    bind(W::Picker, &[&[k(K::Enter)]], A::Confirm, "Picker", "open"),
    bind(
        W::Picker,
        &[&[k(K::Backspace)]],
        A::Backspace,
        "Picker",
        "delete back",
    ),
    bind(W::Picker, &[&[k(K::Esc)]], A::Escape, "Picker", "close"),
    // ----- the command and search line -----
    bind(
        W::Input,
        &[&[k(K::Enter)]],
        A::Confirm,
        "Command line",
        "run, or keep the match",
    ),
    bind(
        W::Input,
        &[&[k(K::Backspace)]],
        A::Backspace,
        "Command line",
        "delete back",
    ),
    bind(
        W::Input,
        &[&[k(K::Tab)]],
        A::CompleteNext,
        "Command line",
        "next command completion",
    ),
    bind(
        W::Input,
        &[&[k(K::BackTab)]],
        A::CompletePrevious,
        "Command line",
        "previous command completion",
    ),
    bind(
        W::Input,
        &[&[k(K::Esc)]],
        A::Escape,
        "Command line",
        "cancel",
    ),
];

/// The prefixes that are submenus, with the word the parent menu and the
/// breadcrumb row name them by (ADR 0049, ADR 0056).
const SUBMENUS: &[(Keys, &str)] = &[
    (&[c(' '), c('f')], "open files"),
    (&[c(' '), c('F')], "files"),
    (&[c(' '), c('p')], "panes"),
    (&[c(' '), c('t')], "threads"),
    (&[c(' '), c('T')], "thread list"),
    (&[c(' '), c('v')], "view"),
    (&[c(' '), c('d')], "diff"),
    (&[c(' '), c('j')], "jump"),
];

/// The word `typed` is a submenu for, when it is one.
fn submenu_word(typed: &[Chord]) -> Option<&'static str> {
    SUBMENUS
        .iter()
        .find(|(keys, _)| *keys == typed)
        .map(|(_, word)| *word)
}

/// The breadcrumb row of the which-key menu: the prefix as it is
/// spelled, then the submenu's word (`Space t · threads`).
#[must_use]
pub(crate) fn menu_title(typed: &[Chord]) -> String {
    match submenu_word(typed) {
        Some(word) => format!("{} · {word}", spell(typed)),
        None => spell(typed),
    }
}

/// `Ctrl` letters zellij's lock mode owns; the viewer never binds them.
#[cfg(test)]
pub(crate) const ZELLIJ_LOCKS: [char; 9] = ['g', 'p', 't', 'n', 'h', 's', 'o', 'q', 'b'];

/// What a typed sequence is on one surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Match {
    /// A binding fires.
    Exact(Action),
    /// A longer binding starts this way; wait for more.
    Prefix,
    /// Nothing starts this way.
    Miss,
}

/// The bindings that apply on `place`: its own, then `Any` when it takes
/// them.
fn applicable(place: Where) -> impl Iterator<Item = &'static Binding> {
    BINDINGS
        .iter()
        .filter(move |b| b.place == place || (b.place == Where::Any && place.takes_any()))
}

/// Look `typed` up on `place`.
#[must_use]
pub(crate) fn lookup(place: Where, typed: &[Chord]) -> Match {
    let mut prefix = false;
    for binding in applicable(place) {
        for keys in binding.keys {
            if *keys == typed {
                return Match::Exact(binding.action);
            }
            if keys.len() > typed.len() && keys.starts_with(typed) {
                prefix = true;
            }
        }
    }
    if prefix { Match::Prefix } else { Match::Miss }
}

/// `label` without the `word: ` that names the submenu `typed` is,
/// which the menu's breadcrumb row already carries.
fn strip_submenu_word<'a>(typed: &[Chord], label: &'a str) -> &'a str {
    submenu_word(typed)
        .and_then(|word| label.strip_prefix(word)?.strip_prefix(": "))
        .unwrap_or(label)
}

/// The which-key entries for `typed` on `place` with the table's
/// labels: the next key of every binding that continues it, in table
/// order. The app draws them through `App::which_key`, which relabels
/// a toggle with what pressing it does now (ADR 0068).
#[cfg(test)]
#[must_use]
pub(crate) fn menu(place: Where, typed: &[Chord]) -> Vec<(String, String)> {
    menu_entries(place, typed, |_| None)
        .into_iter()
        .map(|(chord, label)| (chord.to_string(), label))
        .collect()
}

/// The which-key entries as chords, so a click on a drawn entry can be
/// the key it shows typed (ADR 0050). `relabel` may give an action's
/// entry a live label in place of the table's (ADR 0068).
#[cfg(test)]
#[must_use]
pub(crate) fn menu_entries(
    place: Where,
    typed: &[Chord],
    relabel: impl Fn(Action) -> Option<&'static str>,
) -> Vec<(Chord, String)> {
    menu_entries_with_sections(place, typed, relabel)
        .into_iter()
        .map(|(chord, label, _)| (chord, label))
        .collect()
}

/// One actionable entry in a semantic which-key section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuEntry {
    chord: Chord,
    label: String,
}

impl MenuEntry {
    #[must_use]
    pub(crate) const fn chord(&self) -> Chord {
        self.chord
    }

    #[must_use]
    pub(crate) fn key(&self) -> String {
        self.chord.to_string()
    }

    #[must_use]
    pub(crate) fn label(&self) -> &str {
        &self.label
    }
}

/// One semantic group in a which-key card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuSection {
    entries: Vec<MenuEntry>,
}

impl MenuSection {
    #[must_use]
    pub(crate) fn entries(&self) -> &[MenuEntry] {
        &self.entries
    }
}

/// Which-key sections in binding-table order.
#[must_use]
pub(crate) fn menu_sections(
    place: Where,
    typed: &[Chord],
    relabel: impl Fn(Action) -> Option<&'static str>,
) -> Vec<MenuSection> {
    let entries = menu_entries_with_sections(place, typed, relabel);
    let mut sections: Vec<(u8, MenuSection)> = Vec::new();
    for (chord, label, section) in entries {
        let section = if typed.len() > 1 { section } else { 0 };
        if sections
            .last()
            .is_none_or(|(current, _)| *current != section)
        {
            sections.push((
                section,
                MenuSection {
                    entries: Vec::new(),
                },
            ));
        }
        if let Some((_, current)) = sections.last_mut() {
            current.entries.push(MenuEntry { chord, label });
        }
    }
    sections.into_iter().map(|(_, section)| section).collect()
}

fn menu_entries_with_sections(
    place: Where,
    typed: &[Chord],
    relabel: impl Fn(Action) -> Option<&'static str>,
) -> Vec<(Chord, String, u8)> {
    let mut entries: Vec<(Chord, String, u8)> = Vec::new();
    for binding in applicable(place) {
        for keys in binding.keys {
            if keys.len() > typed.len() && keys.starts_with(typed) {
                let next = keys[typed.len()];
                if !entries.iter().any(|(key, _, _)| *key == next) {
                    let label = if keys.len() == typed.len() + 1 {
                        // The breadcrumb row already names the submenu,
                        // so an entry inside one does not repeat it.
                        relabel(binding.action)
                            .unwrap_or_else(|| strip_submenu_word(typed, binding.label))
                            .to_owned()
                    } else {
                        // A submenu is named after where it leads.
                        let word = submenu_word(&keys[..=typed.len()]).unwrap_or("more");
                        format!("{word}…")
                    };
                    entries.push((next, label, binding.menu_section));
                }
            }
        }
    }
    entries
}

/// How `action` is spelled on `place`, for a hint bar: its first
/// sequence there, else its first `Any` sequence.
#[must_use]
pub(crate) fn hint(place: Where, action: Action) -> Option<String> {
    first_keys(place, action).map(spell)
}

/// The sequence `hint` spells: the first one bound on `place`, else the
/// first `Any` one when the place takes those. A menu entry keeps the
/// chords so a typed key can be matched against them (ADR 0050).
#[must_use]
pub(crate) fn first_keys(place: Where, action: Action) -> Option<Keys> {
    let own = BINDINGS
        .iter()
        .find(|b| b.place == place && b.action == action);
    let any = || {
        place
            .takes_any()
            .then(|| {
                BINDINGS
                    .iter()
                    .find(|b| b.place == Where::Any && b.action == action)
            })
            .flatten()
    };
    own.or_else(any).and_then(|b| b.keys.first()).copied()
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{
        Action, BINDINGS, Chord, Key, Match, Where, ZELLIJ_LOCKS, c, hint, k, lookup, menu,
        menu_sections, menu_spell, shift, spell,
    };

    #[test]
    fn shift_is_retained_for_arrows_and_enter() {
        let shifted = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        let backtab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(Chord::from_event(shifted), Some(k(Key::BackTab)));
        assert_eq!(Chord::from_event(backtab), Some(k(Key::BackTab)));
        assert_eq!(
            Chord::from_event(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            Some(c('A'))
        );
        assert_eq!(
            Chord::from_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
            Some(shift(Key::Enter))
        );
        for key in [Key::Up, Key::Down, Key::Left, Key::Right] {
            let code = match key {
                Key::Up => KeyCode::Up,
                Key::Down => KeyCode::Down,
                Key::Left => KeyCode::Left,
                Key::Right => KeyCode::Right,
                _ => unreachable!("the test lists only arrow keys"),
            };
            assert_eq!(
                Chord::from_event(KeyEvent::new(code, KeyModifiers::SHIFT)),
                Some(shift(key))
            );
        }
    }

    #[test]
    fn shifted_arrows_and_hjkl_share_comparison_navigation() {
        for (keys, action) in [
            ([c('J')], Action::ChangeNext),
            ([shift(Key::Down)], Action::ChangeNext),
            ([c('K')], Action::ChangePrev),
            ([shift(Key::Up)], Action::ChangePrev),
            ([c('L')], Action::ChangeFileNext),
            ([shift(Key::Right)], Action::ChangeFileNext),
            ([c('H')], Action::ChangeFilePrev),
            ([shift(Key::Left)], Action::ChangeFilePrev),
        ] {
            assert_eq!(lookup(Where::View, &keys), Match::Exact(action));
        }
    }

    const PANES: [Where; 4] = [Where::View, Where::Tree, Where::ThreadsPane, Where::Review];

    #[test]
    fn goto_menu_is_navigation_only_and_policy_keys_use_space() {
        assert_eq!(
            menu(Where::View, &[c('g')]),
            [
                ("g", "goto top"),
                ("e", "goto bottom"),
                ("l", "goto line end"),
                ("h", "goto line start"),
                ("f", "open linked file/URL"),
            ]
            .map(|(key, label)| (key.to_owned(), label.to_owned()))
        );
        for suffix in ['s', 'd', 'D', 'y', 'x'] {
            assert_eq!(lookup(Where::View, &[c('g'), c(suffix)]), Match::Miss);
        }
        for place in PANES {
            for (keys, action) in [
                ([c(' '), c('v'), c('s')], Action::SourceView),
                ([c(' '), c('d'), c('s')], Action::DiffStandard),
                ([c(' '), c('d'), c('u')], Action::DiffUnified),
                ([c(' '), c('d'), c('o')], Action::DiffOff),
                ([c(' '), c('d'), c('b')], Action::ComparisonBase),
                ([c(' '), c('d'), c('d')], Action::ComparisonHeadWorkingTree),
                ([c(' '), c('d'), c('r')], Action::ComparisonManage),
            ] {
                assert_eq!(lookup(place, &keys), Match::Exact(action));
            }
            assert_eq!(lookup(place, &[c(' '), c('d'), c('x')]), Match::Miss);
        }
        assert_eq!(lookup(Where::View, &[c('b')]), Match::Miss);
    }

    /// An action nobody can press is dead code the table would hide.
    #[test]
    fn every_action_is_bound() {
        for action in Action::ALL {
            assert!(
                BINDINGS.iter().any(|b| b.action == *action),
                "{action:?} has no binding"
            );
        }
    }

    /// On one surface a sequence fires one thing, and no sequence is the
    /// start of another: `g` could not be both a key and a prefix.
    #[test]
    fn no_surface_has_two_meanings_for_one_sequence() {
        let places = [
            Where::View,
            Where::Tree,
            Where::ThreadsPane,
            Where::Review,
            Where::Draft,
            Where::Picker,
            Where::Input,
        ];
        for place in places {
            let sequences: Vec<(&[Chord], Action)> = super::applicable(place)
                .flat_map(|b| b.keys.iter().map(move |keys| (*keys, b.action)))
                .collect();
            for (i, (keys, action)) in sequences.iter().enumerate() {
                for (other, other_action) in &sequences[i + 1..] {
                    assert!(
                        !(keys == other && action == other_action),
                        "{place:?} binds {} twice",
                        spell(keys)
                    );
                    assert!(
                        keys != other,
                        "{place:?} binds {} to both {action:?} and {other_action:?}",
                        spell(keys)
                    );
                    assert!(
                        !(other.starts_with(keys) || keys.starts_with(other)),
                        "{place:?}: {} is a prefix of {}",
                        spell(keys),
                        spell(other)
                    );
                }
            }
        }
    }

    /// A zellij user has the lock chords taken before the viewer sees them.
    #[test]
    fn no_binding_uses_a_zellij_lock_chord() {
        for binding in BINDINGS {
            for keys in binding.keys {
                for chord in *keys {
                    assert!(
                        !(chord.ctrl
                            && matches!(chord.key, Key::Char(ch) if ZELLIJ_LOCKS.contains(&ch))),
                        "{:?} binds {}, a zellij lock",
                        binding.action,
                        spell(keys)
                    );
                }
            }
        }
    }

    /// Escape and the command line exist on every pane; the popups take
    /// their own Escape and never the Space menu.
    #[test]
    fn every_pane_can_leave_and_command() {
        for place in PANES {
            assert_eq!(
                lookup(place, &[k(Key::Esc)]),
                Match::Exact(Action::Escape),
                "{place:?}"
            );
            assert_eq!(
                lookup(place, &[c(':')]),
                Match::Exact(Action::CommandLine),
                "{place:?}"
            );
            assert_eq!(lookup(place, &[c(' ')]), Match::Prefix, "{place:?}");
        }
        for place in [Where::Draft, Where::Picker, Where::Input] {
            assert_eq!(lookup(place, &[k(Key::Esc)]), Match::Exact(Action::Escape));
            assert_eq!(lookup(place, &[c(' ')]), Match::Miss, "{place:?}");
        }
    }

    /// The menu after `Space` lists each entry once with its next key,
    /// and the submenus open under `f`, `F`, `p`, `t`, `T`, `v`, and `d`
    /// (ADR 0049, ADR 0056, ADR 0060).
    #[test]
    fn menus_come_from_the_table() {
        let space = menu(Where::View, &[c(' ')]);
        assert!(
            space
                .iter()
                .any(|(key, label)| key == "?" && label == "view keymap")
        );
        for (key, word) in [
            ("f", "open files…"),
            ("F", "files…"),
            ("p", "panes…"),
            ("t", "threads…"),
            ("T", "thread list…"),
            ("v", "view…"),
            ("d", "diff…"),
        ] {
            let entries: Vec<&str> = space
                .iter()
                .filter(|(k, _)| k == key)
                .map(|(_, label)| label.as_str())
                .collect();
            assert_eq!(entries, [word], "Space {key} is one submenu entry");
        }
        let keys = |place, typed: &[Chord]| {
            menu(place, typed)
                .into_iter()
                .map(|(key, _)| key)
                .collect::<Vec<_>>()
        };
        assert_eq!(lookup(Where::View, &[c(' '), c('j')]), Match::Miss);
        assert_eq!(lookup(Where::View, &[c(']'), c('f')]), Match::Miss);
        assert_eq!(lookup(Where::View, &[c('['), c('f')]), Match::Miss);
        assert_eq!(
            keys(Where::View, &[c(' '), c('t')]),
            ["R", "h", "a", "A", "c", "r", "e", "d", "f"]
        );
        assert_eq!(keys(Where::View, &[c(' '), c('T')]), ["s", "x"]);
        assert_eq!(keys(Where::Review, &[c(' '), c('v')]), ["s", "t", "r"]);
        assert_eq!(
            keys(Where::Review, &[c(' '), c('d')]),
            ["s", "u", "o", "b", "t", "d", "l", "c", "p", "r", "w"]
        );
        assert_eq!(keys(Where::View, &[c(' '), c('F')]), ["c", "o", "u", "i"]);
        assert_eq!(keys(Where::View, &[c(' '), c('f')]), ["f", "i", "r"]);
        assert_eq!(lookup(Where::View, &[c(' '), c('c')]), Match::Miss);
        assert_eq!(lookup(Where::View, &[c(' '), c('w')]), Match::Miss);
        assert_eq!(
            lookup(Where::View, &[c('w')]),
            Match::Exact(Action::WindowNext)
        );
        assert_eq!(
            lookup(Where::View, &[c('W')]),
            Match::Exact(Action::WindowPrev)
        );
        assert_eq!(
            lookup(Where::View, &[c('F')]),
            Match::Exact(Action::WindowFiles)
        );
        assert_eq!(
            lookup(Where::View, &[c('T')]),
            Match::Exact(Action::WindowThreads)
        );
        assert_eq!(
            lookup(Where::View, &[c(' '), c('d'), c('w')]),
            Match::Exact(Action::ComparisonWhitespace)
        );
        assert_eq!(keys(Where::View, &[c(' '), c('p')]), ["f", "s", "m", "t"]);
        assert_eq!(lookup(Where::View, &[c(' '), c('a')]), Match::Miss);
        assert_eq!(lookup(Where::ThreadsPane, &[c(' '), c(' ')]), Match::Miss);
        assert!(menu(Where::Draft, &[c(' ')]).is_empty());
    }

    #[test]
    fn pane_settings_share_local_and_remote_suffixes() {
        for (suffix, action) in [
            ('c', Action::FilesChanged),
            ('o', Action::FilesReviews),
            ('u', Action::FilesUntracked),
            ('i', Action::FilesIgnored),
        ] {
            assert_eq!(lookup(Where::Tree, &[c(suffix)]), Match::Exact(action));
            assert_eq!(
                lookup(Where::View, &[c(' '), c('F'), c(suffix)]),
                Match::Exact(action)
            );
        }
        for (suffix, action) in [('s', Action::PaneScope), ('x', Action::ReviewResolved)] {
            assert_eq!(
                lookup(Where::ThreadsPane, &[c(suffix)]),
                Match::Exact(action)
            );
            assert_eq!(
                lookup(Where::View, &[c(' '), c('T'), c(suffix)]),
                Match::Exact(action)
            );
        }
    }

    #[test]
    fn mixed_submenus_keep_semantic_sections() {
        let sections = menu_sections(Where::View, &[c(' '), c('d')], |_| None);
        assert_eq!(
            sections
                .iter()
                .map(|section| section.entries().len())
                .collect::<Vec<_>>(),
            [3, 5, 2, 1]
        );
    }

    /// A submenu's entries drop the word the breadcrumb already says:
    /// `Space v r` reads "toggle resolved stubs", not
    /// "view: toggle resolved stubs".
    #[test]
    fn a_submenu_entry_does_not_repeat_the_submenu_word() {
        for (prefix, word) in [
            (c('f'), "open files"),
            (c('F'), "files"),
            (c('p'), "panes"),
            (c('t'), "threads"),
            (c('T'), "thread list"),
            (c('v'), "view"),
            (c('d'), "diff"),
            (c('j'), "jump"),
        ] {
            for (key, label) in menu(Where::View, &[c(' '), prefix]) {
                assert!(
                    !label.starts_with(&format!("{word}: ")),
                    "{key} still says {word}: {label}"
                );
            }
        }
        let stub = menu(Where::View, &[c(' '), c('v')]);
        assert!(
            stub.iter()
                .any(|(key, label)| key == "r" && label == "toggle resolved stubs"),
            "{stub:?}"
        );
    }

    /// The resolved-stub shortcut moved to `Space v r` without retaining
    /// the old `Space v x` sequence.
    #[test]
    fn resolved_stub_shortcut_is_migrated_without_an_alias() {
        for place in PANES {
            assert_eq!(
                lookup(place, &[c(' '), c('v'), c('r')]),
                Match::Exact(Action::StubResolvedToggle),
                "{place:?}"
            );
            assert_eq!(
                lookup(place, &[c(' '), c('v'), c('x')]),
                Match::Miss,
                "{place:?}"
            );
        }
        assert_eq!(
            menu(Where::View, &[c(' '), c('v')]),
            [
                ("s", "source view"),
                ("t", "toggle thread stubs"),
                ("r", "toggle resolved stubs"),
            ]
            .map(|(key, label)| (key.to_owned(), label.to_owned()))
        );
    }

    /// Every prefix under `Space` that leads further is a named submenu,
    /// so the parent entry and the breadcrumb never fall back to "more".
    #[test]
    fn every_space_submenu_is_named() {
        for binding in BINDINGS {
            for keys in binding.keys {
                if keys.len() > 2 && keys[0] == c(' ') {
                    assert!(
                        super::submenu_word(&keys[..2]).is_some(),
                        "{} has no submenu name",
                        spell(&keys[..2])
                    );
                }
            }
        }
        assert_eq!(super::menu_title(&[c(' ')]), "Space");
        assert_eq!(super::menu_title(&[c(' '), c('t')]), "Space t · threads");
        assert_eq!(
            super::menu_title(&[c(' '), c('T')]),
            "Space T · thread list"
        );
        assert_eq!(super::menu_title(&[c('g')]), "g");
    }

    /// The guide's key section names only keys the table binds: every
    /// backticked token in its key columns is a bound sequence or the start
    /// of one (a command, `:x`, is checked elsewhere). The prose around
    /// the tables stays hand-written.
    #[test]
    fn guide_key_tables_name_only_bound_keys() -> std::io::Result<()> {
        let guide = fathomable_testing::repo_file("docs/guide.md");
        let text = std::fs::read_to_string(guide)?;
        let start = text
            .find("### Essential keys")
            .ok_or(std::io::ErrorKind::NotFound)?;
        let end = text[start..]
            .find("\n### ")
            .map_or(text.len(), |i| start + i);
        let mut bound: Vec<String> = BINDINGS
            .iter()
            .flat_map(|b| b.keys.iter())
            .map(|keys| spell(keys).replace(' ', ""))
            .collect();
        bound.sort();
        bound.dedup();
        let mut checked = 0;
        for line in text[start..end].lines() {
            let Some(rest) = line.strip_prefix("| ") else {
                continue;
            };
            let Some((keys, _)) = rest.split_once(" | ") else {
                continue;
            };
            if keys.starts_with("Keys") || keys.starts_with("---") {
                continue;
            }
            for token in keys.split('`').skip(1).step_by(2) {
                if token.starts_with(':') || token.starts_with('$') || token == "HEAD" {
                    continue;
                }
                let want = token.replace(' ', "");
                assert!(
                    bound.iter().any(|k| k.starts_with(&want)),
                    "guide key table names `{token}`, which no binding starts with"
                );
                checked += 1;
            }
        }
        assert!(checked > 0, "the guide's key tables were not found");
        Ok(())
    }

    /// Spelling: bare characters run together, everything else is spaced.
    #[test]
    fn sequences_are_spelled_the_way_the_guide_writes_them() {
        assert_eq!(spell(&[c('g'), c('g')]), "gg");
        assert_eq!(spell(&[c(']'), c('c')]), "]c");
        assert_eq!(spell(&[c(' '), c('d'), c('s')]), "Space d s");
        assert_eq!(menu_spell(&[c(' '), c('j'), c('j')]), "Sp j j");
        assert_eq!(lookup(Where::Any, &[c(' '), c('j'), c('a')]), Match::Miss);
        assert_eq!(spell(&[super::ctrl('d')]), "Ctrl-d");
        assert_eq!(spell(&[shift(Key::Enter)]), "Shift-Enter");
        assert_eq!(spell(&[super::alt(Key::Enter)]), "Alt-Enter");
        assert_eq!(hint(Where::Review, Action::Reply).as_deref(), Some("c"));
        assert_eq!(hint(Where::Tree, Action::CopyPath).as_deref(), Some("y"));
        assert_eq!(
            hint(Where::Tree, Action::CopyFullPath).as_deref(),
            Some("Y")
        );
        assert_eq!(
            hint(Where::View, Action::Reply).as_deref(),
            Some("Space t r")
        );
        assert_eq!(
            lookup(Where::View, &[c('r')]),
            Match::Exact(Action::ToggleResolved)
        );
        assert_eq!(
            lookup(Where::View, &[c('R')]),
            Match::Exact(Action::ToggleAutoResolve)
        );
        assert_eq!(
            lookup(Where::View, &[c('a')]),
            Match::Exact(Action::ArchiveThread)
        );
        assert_eq!(
            lookup(Where::ThreadsPane, &[c('a')]),
            Match::Exact(Action::ArchiveThread)
        );
        assert_eq!(
            lookup(Where::ThreadsPane, &[c('u')]),
            Match::Exact(Action::RestoreThread)
        );
        assert_eq!(lookup(Where::View, &[c('t')]), Match::Exact(Action::Review));
        assert_eq!(
            lookup(Where::View, &[c('f')]),
            Match::Exact(Action::FileView)
        );
        assert_eq!(
            lookup(Where::Review, &[c('s')]),
            Match::Exact(Action::FileOnly)
        );
        for place in [Where::Draft, Where::Picker, Where::Input] {
            assert_eq!(lookup(place, &[c('q')]), Match::Miss);
            assert_eq!(lookup(place, &[c('t')]), Match::Miss);
            assert_eq!(lookup(place, &[c('f')]), Match::Miss);
            assert_eq!(lookup(place, &[c('r')]), Match::Miss);
            assert_eq!(lookup(place, &[c('R')]), Match::Miss);
        }
        for place in PANES {
            assert_eq!(lookup(place, &[c('q')]), Match::Exact(Action::ConfirmQuit));
        }
        assert_eq!(
            lookup(
                Where::Draft,
                &[Chord {
                    key: Key::Enter,
                    ctrl: true,
                    alt: false,
                    shift: false,
                }]
            ),
            Match::Exact(Action::SubmitAutoResolve)
        );
        assert_eq!(
            lookup(Where::Draft, &[shift(Key::Enter)]),
            Match::Exact(Action::Newline)
        );
        assert_eq!(
            lookup(Where::Draft, &[super::alt(Key::Enter)]),
            Match::Exact(Action::Newline)
        );
        assert_eq!(
            lookup(Where::Draft, &[k(Key::Enter)]),
            Match::Exact(Action::Confirm)
        );
        assert_eq!(
            hint(Where::Review, Action::CommandLine).as_deref(),
            Some(":")
        );
        assert_eq!(hint(Where::Draft, Action::CommandLine), None);
    }
}
