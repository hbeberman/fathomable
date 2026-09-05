// @okf-doc: /decisions/0045-bindings-are-data.md
//! The key bindings as one table: what each key does where, and the
//! words the help popup, the menus, and the hint bars show for it.
//!
//! A [`Binding`] pairs one [`Action`] with the key sequences that fire it
//! in one [`Where`]. Dispatch looks a typed sequence up with [`lookup`];
//! a sequence that is the start of a longer binding is a prefix and the
//! viewer waits for the rest, showing [`menu`] entries meanwhile. The
//! help popup renders [`help`], and a pane header asks [`hint`] how a key
//! is spelled, so no surface can name a key the table does not bind.

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

/// One key press: a [`Key`] with its modifiers. Shift is not a modifier
/// here; a shifted letter arrives as its uppercase character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Chord {
    pub(crate) key: Key,
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
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
        })
    }

    /// Whether this is a bare character: no modifier, not the space bar.
    #[must_use]
    pub(crate) fn is_plain_char(self) -> bool {
        !self.ctrl && !self.alt && matches!(self.key, Key::Char(ch) if ch != ' ')
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
        write!(f, "{}", self.key)
    }
}

/// A plain character key.
const fn c(ch: char) -> Chord {
    Chord {
        key: Key::Char(ch),
        ctrl: false,
        alt: false,
    }
}

/// A key with Ctrl held.
const fn ctrl(ch: char) -> Chord {
    Chord {
        key: Key::Char(ch),
        ctrl: true,
        alt: false,
    }
}

/// A key with Alt held.
const fn alt(key: Key) -> Chord {
    Chord {
        key,
        ctrl: false,
        alt: true,
    }
}

/// A named key without modifiers.
const fn k(key: Key) -> Chord {
    Chord {
        key,
        ctrl: false,
        alt: false,
    }
}

/// A key sequence: one chord, or a prefix and what follows it.
pub(crate) type Keys = &'static [Chord];

/// How a sequence is written: bare characters run together (`gg`, `]c`),
/// anything else is space-separated (`Space j a`, `Ctrl-d`).
#[must_use]
pub(crate) fn spell(keys: &[Chord]) -> String {
    let separator = if keys.iter().all(|chord| chord.is_plain_char()) {
        ""
    } else {
        " "
    };
    keys.iter()
        .map(ToString::to_string)
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
    /// The file, recent, or wake picker.
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
    CopyLink,
    OpenLink,
    GotoFile,
    CopyPath,
    Comment,
    NewThread,
    WaitingNext,
    WaitingPrev,
    SourceView,
    DiffHead,
    DiffSeen,
    CheckpointFile,
    CheckpointWorkspace,
    DiffCheckpoint,
    DiffCommit,
    DiffBase,
    DiffTarget,
    DiffWhitespace,
    StubsToggle,
    HunkNext,
    HunkPrev,
    DirtyNext,
    DirtyPrev,
    ChangeNext,
    ChangePrev,
    JumpNewest,
    AutoJumpToggle,
    JumpBack,
    JumpForward,
    CommandLine,
    Escape,
    Confirm,
    TreeToggle,
    PickFile,
    PickAnyFile,
    PickRecent,
    Review,
    ThreadsPaneToggle,
    WindowLeft,
    WindowDown,
    WindowUp,
    WindowRight,
    WindowNext,
    WindowFiles,
    WindowThreads,
    PaneScope,
    ReviewResolved,
    ReviewSort,
    Wake,
    Help,
    ThreadNext,
    ThreadPrev,
    ThreadNextAcross,
    ThreadPrevAcross,
    Reply,
    EditMessage,
    EditNewestOwn,
    StubResolvedToggle,
    ToggleResolved,
    Delete,
    DeleteThread,
    Fold,
    FileOnly,
    Newline,
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
    }
}

use Action as A;
use Key as K;
use Where as W;

/// Every binding. Order is the help popup's order.
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
        "left, wrapping onto the row above",
    ),
    bind(
        W::View,
        &[&[c('l')], &[k(K::Right)]],
        A::MoveRight,
        "Move",
        "right, wrapping onto the row below",
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
    bind(W::View, &[&[c('g'), c('g')]], A::Top, "Move", "top"),
    bind(
        W::View,
        &[&[c('g'), c('e')], &[c('G')]],
        A::Bottom,
        "Move",
        "bottom",
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
        &[&[c('g'), c('y')]],
        A::CopyLink,
        "Links",
        "copy the link here",
    ),
    bind(
        W::View,
        &[&[c('g'), c('x')]],
        A::OpenLink,
        "Links",
        "open the link here",
    ),
    bind(
        W::View,
        &[&[c('g'), c('f')]],
        A::GotoFile,
        "Links",
        "open the file named here in the viewer, at its line",
    ),
    bind(
        W::View,
        &[&[c('c')]],
        A::Comment,
        "Threads",
        "expand or fold the thread here, else comment on the selection or line",
    ),
    bind(
        W::View,
        &[&[c('C')]],
        A::NewThread,
        "Threads",
        "always start a new thread",
    ),
    bind(
        W::View,
        &[&[c('r')]],
        A::Reply,
        "Threads",
        "reply to the thread here",
    ),
    bind(
        W::View,
        &[&[c('e')]],
        A::EditMessage,
        "Threads",
        "edit the message here, when yours",
    ),
    bind(
        W::View,
        &[&[c('o')]],
        A::ToggleResolved,
        "Threads",
        "resolve or reopen the thread here",
    ),
    bind(
        W::View,
        &[&[c('d'), c('d')]],
        A::Delete,
        "Threads",
        "delete the thread here",
    ),
    bind(
        W::View,
        &[&[c(']'), c('c')]],
        A::ThreadNext,
        "Threads",
        "next thread in the file",
    ),
    bind(
        W::View,
        &[&[c('['), c('c')]],
        A::ThreadPrev,
        "Threads",
        "previous thread in the file",
    ),
    bind(
        W::View,
        &[&[c(']'), c('C')]],
        A::ThreadNextAcross,
        "Threads",
        "next thread across the workspace",
    ),
    bind(
        W::View,
        &[&[c('['), c('C')]],
        A::ThreadPrevAcross,
        "Threads",
        "previous thread across the workspace",
    ),
    bind(
        W::View,
        &[&[c(']'), c('r')], &[k(K::Tab)]],
        A::WaitingNext,
        "Threads",
        "next thread waiting on you, across files",
    ),
    bind(
        W::View,
        &[&[c('['), c('r')], &[k(K::BackTab)]],
        A::WaitingPrev,
        "Threads",
        "previous thread waiting on you, across files",
    ),
    bind(
        W::View,
        &[&[c('g'), c('s')]],
        A::SourceView,
        "Display",
        "toggle source view",
    ),
    bind(
        W::View,
        &[&[c('g'), c('d')]],
        A::DiffHead,
        "Display",
        "toggle the diff against HEAD",
    ),
    bind(
        W::View,
        &[&[c('g'), c('D')]],
        A::DiffSeen,
        "Display",
        "toggle the diff against last seen",
    ),
    bind(
        W::View,
        &[&[c('b')]],
        A::DiffBase,
        "Display",
        "diff: pick the base",
    ),
    bind(
        W::View,
        &[&[c('t')]],
        A::DiffTarget,
        "Display",
        "diff: pick the target",
    ),
    bind(
        W::View,
        &[&[c('w')]],
        A::DiffWhitespace,
        "Display",
        "diff: ignore whitespace",
    ),
    bind(
        W::View,
        &[&[c(']'), c('g')]],
        A::HunkNext,
        "Git",
        "next hunk, across files",
    ),
    bind(
        W::View,
        &[&[c('['), c('g')]],
        A::HunkPrev,
        "Git",
        "previous hunk, across files",
    ),
    bind(
        W::View,
        &[&[c(']'), c('G')]],
        A::DirtyNext,
        "Git",
        "next uncommitted file",
    ),
    bind(
        W::View,
        &[&[c('['), c('G')]],
        A::DirtyPrev,
        "Git",
        "previous uncommitted file",
    ),
    bind(
        W::View,
        &[&[c(']'), c('f')]],
        A::ChangeNext,
        "Changes",
        "next changed file",
    ),
    bind(
        W::View,
        &[&[c('['), c('f')]],
        A::ChangePrev,
        "Changes",
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
        &[&[c(' '), c('f')]],
        A::PickFile,
        "Space menu",
        "open file",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('F'), c('i')]],
        A::PickAnyFile,
        "Space menu",
        "files: open file incl. ignored",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('F'), c('r')]],
        A::PickRecent,
        "Space menu",
        "files: recent files",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('r')]],
        A::Review,
        "Space menu",
        "review list",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('w'), c('h')]],
        A::WindowLeft,
        "Space menu",
        "window: left",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('w'), c('j')]],
        A::WindowDown,
        "Space menu",
        "window: down",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('w'), c('k')]],
        A::WindowUp,
        "Space menu",
        "window: up",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('w'), c('l')]],
        A::WindowRight,
        "Space menu",
        "window: right",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('w'), c('w')], &[c(' '), c(' ')]],
        A::WindowNext,
        "Space menu",
        "next pane",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('w'), c('f')]],
        A::WindowFiles,
        "Space menu",
        "window: files pane",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('w'), c('t')]],
        A::WindowThreads,
        "Space menu",
        "window: threads pane",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('p'), c('f')]],
        A::TreeToggle,
        "Space menu",
        "panes: toggle files pane",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('p'), c('t')]],
        A::ThreadsPaneToggle,
        "Space menu",
        "panes: toggle threads pane",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('c')]],
        A::NewThread,
        "Space menu",
        "threads: new thread",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('r')]],
        A::Reply,
        "Space menu",
        "threads: reply",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('o')]],
        A::ToggleResolved,
        "Space menu",
        "threads: resolve or reopen",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('e')]],
        A::EditNewestOwn,
        "Space menu",
        "threads: edit message",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('d')]],
        A::DeleteThread,
        "Space menu",
        "threads: delete thread",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('v'), c('s')]],
        A::SourceView,
        "Space menu",
        "view: source view",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('v'), c('t')]],
        A::StubsToggle,
        "Space menu",
        "view: toggle thread stubs",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('v'), c('x')]],
        A::StubResolvedToggle,
        "Space menu",
        "view: toggle resolved stubs",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('d')]],
        A::DiffHead,
        "Space menu",
        "diff: vs HEAD",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('D')]],
        A::DiffSeen,
        "Space menu",
        "diff: vs last seen",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('r')]],
        A::DiffCheckpoint,
        "Space menu",
        "diff: checkpoint diff",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('g')]],
        A::DiffCommit,
        "Space menu",
        "diff: vs commit…",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('b')]],
        A::DiffBase,
        "Space menu",
        "diff: pick base…",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('t')]],
        A::DiffTarget,
        "Space menu",
        "diff: pick target…",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('c')]],
        A::CheckpointFile,
        "Space menu",
        "diff: checkpoint file",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('C')]],
        A::CheckpointWorkspace,
        "Space menu",
        "diff: checkpoint workspace",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('d'), c('w')]],
        A::DiffWhitespace,
        "Space menu",
        "diff: ignore whitespace",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('j'), c('j')]],
        A::JumpNewest,
        "Space menu",
        "jump: newest change",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('j'), c('a')]],
        A::AutoJumpToggle,
        "Space menu",
        "jump: auto-jump",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('a'), c('w')]],
        A::Wake,
        "Space menu",
        "agent: wake",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('?')]],
        A::Help,
        "Space menu",
        "all keys",
    ),
    // ----- the tree -----
    bind(
        W::Tree,
        &[&[c('j')], &[k(K::Down)]],
        A::MoveDown,
        "Tree",
        "down, showing the file",
    ),
    bind(
        W::Tree,
        &[&[c('k')], &[k(K::Up)]],
        A::MoveUp,
        "Tree",
        "up, showing the file",
    ),
    bind(
        W::Tree,
        &[&[c('h')], &[k(K::Left)]],
        A::MoveLeft,
        "Tree",
        "collapse",
    ),
    bind(
        W::Tree,
        &[&[c('l')], &[k(K::Right)]],
        A::MoveRight,
        "Tree",
        "expand, or open",
    ),
    bind(
        W::Tree,
        &[&[k(K::Enter)]],
        A::Confirm,
        "Tree",
        "open and focus the text",
    ),
    bind(W::Tree, &[&[c('g'), c('g')]], A::Top, "Tree", "top"),
    bind(
        W::Tree,
        &[&[c('g'), c('e')], &[c('G')]],
        A::Bottom,
        "Tree",
        "bottom",
    ),
    bind(W::Tree, &[&[c('y')]], A::CopyPath, "Tree", "copy the path"),
    bind(
        W::Tree,
        &[&[k(K::Esc)]],
        A::Escape,
        "Tree",
        "back to the text",
    ),
    // ----- the threads pane -----
    bind(
        W::ThreadsPane,
        &[&[c('j')], &[k(K::Down)]],
        A::MoveDown,
        "Threads pane",
        "next thread; the text follows",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('k')], &[k(K::Up)]],
        A::MoveUp,
        "Threads pane",
        "previous thread; the text follows",
    ),
    bind(
        W::ThreadsPane,
        &[&[k(K::Enter)], &[c('l')], &[k(K::Right)]],
        A::Confirm,
        "Threads pane",
        "open the file with the thread expanded",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('s')]],
        A::PaneScope,
        "Threads pane",
        "this file, or the workspace",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('x')]],
        A::ReviewResolved,
        "Threads pane",
        "show or hide resolved threads",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('r')]],
        A::Reply,
        "Threads pane",
        "reply",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('o')]],
        A::ToggleResolved,
        "Threads pane",
        "resolve or reopen",
    ),
    bind(
        W::ThreadsPane,
        &[&[c('d'), c('d')]],
        A::Delete,
        "Threads pane",
        "delete the thread",
    ),
    bind(
        W::ThreadsPane,
        &[&[k(K::Esc)]],
        A::Escape,
        "Threads pane",
        "back to the text; the pane stays",
    ),
    // ----- the review list -----
    bind(
        W::Review,
        &[&[c('j')], &[k(K::Down)]],
        A::ThreadNext,
        "Review list",
        "next thread",
    ),
    bind(
        W::Review,
        &[&[c('k')], &[k(K::Up)]],
        A::ThreadPrev,
        "Review list",
        "previous thread",
    ),
    bind(
        W::Review,
        &[&[c('l')], &[k(K::Right)]],
        A::MoveDown,
        "Review list",
        "next message",
    ),
    bind(
        W::Review,
        &[&[c('h')], &[k(K::Left)]],
        A::MoveUp,
        "Review list",
        "previous message",
    ),
    bind(
        W::Review,
        &[&[c('g'), c('g')]],
        A::Top,
        "Review list",
        "first thread",
    ),
    bind(
        W::Review,
        &[&[c('g'), c('e')], &[c('G')]],
        A::Bottom,
        "Review list",
        "last thread",
    ),
    bind(
        W::Review,
        &[&[ctrl('d')]],
        A::HalfPageDown,
        "Review list",
        "half a page down",
    ),
    bind(
        W::Review,
        &[&[ctrl('u')]],
        A::HalfPageUp,
        "Review list",
        "half a page up",
    ),
    bind(
        W::Review,
        &[&[k(K::Enter)]],
        A::Confirm,
        "Review list",
        "open the file with the thread expanded on this message",
    ),
    bind(W::Review, &[&[c('r')]], A::Reply, "Review list", "reply"),
    bind(
        W::Review,
        &[&[c('e')]],
        A::EditMessage,
        "Review list",
        "edit your message",
    ),
    bind(
        W::Review,
        &[&[c('o')]],
        A::ToggleResolved,
        "Review list",
        "resolve or reopen",
    ),
    bind(
        W::Review,
        &[&[c('d'), c('d')]],
        A::Delete,
        "Review list",
        "delete the thread",
    ),
    bind(
        W::Review,
        &[&[c('z')]],
        A::Fold,
        "Review list",
        "fold the entry",
    ),
    bind(
        W::Review,
        &[&[c('s')]],
        A::ReviewSort,
        "Review list",
        "newest agent reply first, or by file",
    ),
    bind(
        W::Review,
        &[&[c('x')]],
        A::ReviewResolved,
        "Review list",
        "show or hide resolved threads",
    ),
    bind(
        W::Review,
        &[&[c('f')]],
        A::FileOnly,
        "Review list",
        "only this file",
    ),
    bind(
        W::Review,
        &[&[k(K::Esc)]],
        A::Escape,
        "Review list",
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
        &[
            &[alt(K::Enter)],
            &[Chord {
                key: K::Enter,
                ctrl: true,
                alt: false,
            }],
        ],
        A::Newline,
        "Draft",
        "newline",
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
        &[&[k(K::Esc)]],
        A::Escape,
        "Command line",
        "cancel",
    ),
];

/// The prefixes that are submenus, with the word the parent menu and the
/// breadcrumb row name them by (ADR 0049, ADR 0056).
const SUBMENUS: &[(Keys, &str)] = &[
    (&[c(' '), c('F')], "files"),
    (&[c(' '), c('w')], "window"),
    (&[c(' '), c('p')], "panes"),
    (&[c(' '), c('c')], "threads"),
    (&[c(' '), c('v')], "view"),
    (&[c(' '), c('d')], "diff"),
    (&[c(' '), c('j')], "jump"),
    (&[c(' '), c('a')], "agent"),
];

/// The word `typed` is a submenu for, when it is one.
fn submenu_word(typed: &[Chord]) -> Option<&'static str> {
    SUBMENUS
        .iter()
        .find(|(keys, _)| *keys == typed)
        .map(|(_, word)| *word)
}

/// The breadcrumb row of the which-key menu: the prefix as it is
/// spelled, then the submenu's word (`Space c · threads`).
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

/// The which-key entries for `typed` on `place`: the next key of every
/// binding that continues it, with its label, in table order.
#[must_use]
pub(crate) fn menu(place: Where, typed: &[Chord]) -> Vec<(String, String)> {
    menu_entries(place, typed)
        .into_iter()
        .map(|(chord, label)| (chord.to_string(), label))
        .collect()
}

/// The which-key entries as chords, so a click on a drawn entry can be
/// the key it shows typed (ADR 0050).
#[must_use]
pub(crate) fn menu_entries(place: Where, typed: &[Chord]) -> Vec<(Chord, String)> {
    let mut entries: Vec<(Chord, String)> = Vec::new();
    for binding in applicable(place) {
        for keys in binding.keys {
            if keys.len() > typed.len() && keys.starts_with(typed) {
                let next = keys[typed.len()];
                if !entries.iter().any(|(key, _)| *key == next) {
                    let label = if keys.len() == typed.len() + 1 {
                        // The breadcrumb row already names the submenu,
                        // so an entry inside one does not repeat it.
                        strip_submenu_word(typed, binding.label).to_owned()
                    } else {
                        // A submenu is named after where it leads.
                        let word = submenu_word(&keys[..=typed.len()]).unwrap_or("more");
                        format!("{word}…")
                    };
                    entries.push((next, label));
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

/// The help popup: one row per binding, keys joined by ` / `, under a
/// header row per group.
#[must_use]
pub(crate) fn help() -> Vec<(String, String)> {
    help_rows().into_iter().map(|(_, row)| row).collect()
}

/// The help rows with the binding each one stands for, `None` on a
/// group header, so a click on a row can run it (ADR 0050).
#[must_use]
pub(crate) fn help_rows() -> Vec<(Option<&'static Binding>, (String, String))> {
    let mut rows = Vec::with_capacity(BINDINGS.len() + 16);
    let mut group = "";
    for binding in BINDINGS {
        if binding.group != group {
            group = binding.group;
            rows.push((None, (String::new(), group.to_owned())));
        }
        let keys = binding
            .keys
            .iter()
            .map(|keys| spell(keys))
            .collect::<Vec<_>>()
            .join(" / ");
        rows.push((Some(binding), (keys, binding.label.to_owned())));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{
        Action, BINDINGS, Chord, Key, Match, Where, ZELLIJ_LOCKS, c, help, hint, k, lookup, menu,
        spell,
    };

    const PANES: [Where; 4] = [Where::View, Where::Tree, Where::ThreadsPane, Where::Review];

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
    /// and the submenus open under `F`, `w`, `p`, `c`, `v`, `d`, `j`, and
    /// `a` (ADR 0049, ADR 0056, ADR 0060).
    #[test]
    fn menus_come_from_the_table() {
        let space = menu(Where::View, &[c(' ')]);
        assert!(
            space
                .iter()
                .any(|(key, label)| key == "?" && label == "all keys")
        );
        for (key, word) in [
            ("F", "files…"),
            ("w", "window…"),
            ("p", "panes…"),
            ("c", "threads…"),
            ("v", "view…"),
            ("d", "diff…"),
            ("j", "jump…"),
            ("a", "agent…"),
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
        assert_eq!(keys(Where::Tree, &[c(' '), c('j')]), ["j", "a"]);
        assert_eq!(
            keys(Where::View, &[c(' '), c('c')]),
            ["c", "r", "o", "e", "d"]
        );
        assert_eq!(keys(Where::Review, &[c(' '), c('v')]), ["s", "t", "x"]);
        assert_eq!(
            keys(Where::Review, &[c(' '), c('d')]),
            ["d", "D", "r", "g", "b", "t", "c", "C", "w"]
        );
        assert_eq!(keys(Where::View, &[c(' '), c('F')]), ["i", "r"]);
        assert_eq!(
            keys(Where::View, &[c(' '), c('w')]),
            ["h", "j", "k", "l", "w", "f", "t"]
        );
        assert_eq!(keys(Where::View, &[c(' '), c('p')]), ["f", "t"]);
        assert_eq!(keys(Where::View, &[c(' '), c('a')]), ["w"]);
        assert_eq!(
            lookup(Where::ThreadsPane, &[c(' '), c(' ')]),
            Match::Exact(Action::WindowNext)
        );
        assert!(menu(Where::Draft, &[c(' ')]).is_empty());
    }

    /// A submenu's entries drop the word the breadcrumb already says:
    /// `Space v x` reads "toggle resolved stubs", not
    /// "view: toggle resolved stubs".
    #[test]
    fn a_submenu_entry_does_not_repeat_the_submenu_word() {
        for (prefix, word) in [
            (c('F'), "files"),
            (c('w'), "window"),
            (c('p'), "panes"),
            (c('c'), "threads"),
            (c('v'), "view"),
            (c('d'), "diff"),
            (c('j'), "jump"),
            (c('a'), "agent"),
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
                .any(|(key, label)| key == "x" && label == "toggle resolved stubs"),
            "{stub:?}"
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
        assert_eq!(super::menu_title(&[c(' '), c('c')]), "Space c · threads");
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
            .find("## 3. Keys")
            .ok_or(std::io::ErrorKind::NotFound)?;
        let end = text[start..]
            .find("\n## 4.")
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
                    "guide §3 names `{token}`, which no binding starts with"
                );
                checked += 1;
            }
        }
        assert!(checked > 60, "the guide's key tables were not found");
        Ok(())
    }

    /// Spelling: bare characters run together, everything else is spaced.
    #[test]
    fn sequences_are_spelled_the_way_the_guide_writes_them() {
        assert_eq!(spell(&[c('g'), c('g')]), "gg");
        assert_eq!(spell(&[c(']'), c('c')]), "]c");
        assert_eq!(spell(&[c(' '), c('j'), c('a')]), "Space j a");
        assert_eq!(spell(&[super::ctrl('d')]), "Ctrl-d");
        assert_eq!(spell(&[super::alt(Key::Enter)]), "Alt-Enter");
        assert_eq!(hint(Where::Review, Action::Reply).as_deref(), Some("r"));
        assert_eq!(hint(Where::Tree, Action::CopyPath).as_deref(), Some("y"));
        assert_eq!(hint(Where::View, Action::Reply).as_deref(), Some("r"));
        assert_eq!(
            hint(Where::Review, Action::CommandLine).as_deref(),
            Some(":")
        );
        assert_eq!(hint(Where::Draft, Action::CommandLine), None);
        assert!(help().iter().any(|(keys, _)| keys == "j / Down"));
    }
}
