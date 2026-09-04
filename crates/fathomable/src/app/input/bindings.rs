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
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
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
pub struct Chord {
    pub key: Key,
    pub ctrl: bool,
    pub alt: bool,
}

impl Chord {
    /// The chord a terminal event is, or `None` for a key the viewer
    /// never binds (function keys, media keys).
    #[must_use]
    pub fn from_event(event: KeyEvent) -> Option<Self> {
        let key = match event.code {
            KeyCode::Char(ch) => Key::Char(ch),
            KeyCode::Enter => Key::Enter,
            KeyCode::Esc => Key::Esc,
            KeyCode::Tab => Key::Tab,
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
    pub fn is_plain_char(self) -> bool {
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
pub type Keys = &'static [Chord];

/// How a sequence is written: bare characters run together (`gg`, `]c`),
/// anything else is space-separated (`Space j a`, `Ctrl-d`).
#[must_use]
pub fn spell(keys: &[Chord]) -> String {
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
/// reaches a popup: the comment box, the picker, and the command line
/// take only their own keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where {
    /// The text, in normal or select mode.
    View,
    /// The file tree.
    Tree,
    /// The thread pane under the text.
    ThreadPane,
    /// The rail's threads pane.
    ThreadsPane,
    /// The workspace thread list.
    List,
    /// The comment box.
    Box,
    /// The file, recent, or wake picker.
    Picker,
    /// The `:` and `/` input line.
    Input,
    /// Every pane: the view, the tree, and the three thread surfaces.
    Any,
}

impl Where {
    /// Whether `Any` bindings apply here: in a pane, not a popup.
    #[must_use]
    pub fn takes_any(self) -> bool {
        !matches!(self, Self::Box | Self::Picker | Self::Input)
    }
}

macro_rules! actions {
    ($($(#[$meta:meta])* $name:ident),* $(,)?) => {
        /// What a key does. One action can be bound on several surfaces;
        /// `App::act` gives it that surface's meaning.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Action {
            $($(#[$meta])* $name),*
        }

        impl Action {
            /// Every action, for the test that each one is bound.
            #[cfg(test)]
            pub const ALL: &'static [Self] = &[$(Self::$name),*];
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
    Comment,
    NewThread,
    WaitingNext,
    WaitingPrev,
    SourceView,
    DiffHead,
    DiffSeen,
    HunkNext,
    HunkPrev,
    DirtyNext,
    DirtyPrev,
    ChangeNext,
    ChangePrev,
    JumpNewest,
    AutoJumpToggle,
    ClearChanges,
    JumpBack,
    JumpForward,
    CommandLine,
    Escape,
    Confirm,
    TreeToggleFocus,
    TreeHide,
    TreeRefresh,
    TreeIgnored,
    TreeReveal,
    PickFile,
    PickAnyFile,
    PickRecent,
    ThreadAtCursor,
    ThreadList,
    ThreadsPaneFocus,
    ThreadsPaneHide,
    PaneScope,
    PaneResolved,
    Wake,
    Help,
    ThreadNext,
    ThreadPrev,
    ThreadNextAcross,
    ThreadPrevAcross,
    Reply,
    EditMessage,
    EditNewestOwn,
    StubToggle,
    StubResolvedToggle,
    ToggleResolved,
    Delete,
    DeleteThread,
    Fold,
    FoldResolved,
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
pub struct Binding {
    pub keys: &'static [Keys],
    pub place: Where,
    pub action: Action,
    pub label: &'static str,
    pub group: &'static str,
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
pub const BINDINGS: &[Binding] = &[
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
        "left; at column 0, focus the tree",
    ),
    bind(
        W::View,
        &[&[c('l')], &[k(K::Right)]],
        A::MoveRight,
        "Move",
        "right",
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
        "copy the selection",
    ),
    bind(
        W::View,
        &[&[c('c')]],
        A::Comment,
        "Threads",
        "open the thread here, else comment on the selection or line",
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
        &[&[c(']'), c('r')]],
        A::WaitingNext,
        "Threads",
        "next thread waiting on you, across files",
    ),
    bind(
        W::View,
        &[&[c('['), c('r')]],
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
    // ----- the Space menu -----
    bind(
        W::Any,
        &[&[c(' '), c('e')]],
        A::TreeToggleFocus,
        "Space menu",
        "tree: focus, or return",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('E')]],
        A::TreeHide,
        "Space menu",
        "tree: hide",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('f')]],
        A::PickFile,
        "Space menu",
        "open file",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('F')]],
        A::PickAnyFile,
        "Space menu",
        "open file (incl. ignored)",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('o')]],
        A::PickRecent,
        "Space menu",
        "recent files",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('a')]],
        A::ThreadAtCursor,
        "Space menu",
        "thread pane: open on the thread here, or close",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('A')]],
        A::ThreadList,
        "Space menu",
        "thread list: open, or close",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('t')]],
        A::ThreadsPaneFocus,
        "Space menu",
        "threads pane: focus, or return",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('T')]],
        A::ThreadsPaneHide,
        "Space menu",
        "threads pane: hide",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('r'), c('r')]],
        A::TreeRefresh,
        "Space menu",
        "rail: re-read the tree",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('r'), c('i')]],
        A::TreeIgnored,
        "Space menu",
        "rail: toggle ignored entries",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('r'), c('.')]],
        A::TreeReveal,
        "Space menu",
        "rail: reveal this file in the tree",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('c')]],
        A::StubToggle,
        "Space menu",
        "threads: toggle stub visibility",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('x')]],
        A::StubResolvedToggle,
        "Space menu",
        "threads: toggle resolved stubs",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('n')]],
        A::NewThread,
        "Space menu",
        "threads: new thread on the cursor line",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('r')]],
        A::Reply,
        "Space menu",
        "threads: reply to the thread here",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('o')]],
        A::ToggleResolved,
        "Space menu",
        "threads: resolve or reopen the thread here",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('e')]],
        A::EditNewestOwn,
        "Space menu",
        "threads: edit your newest message in the thread here",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('c'), c('d')]],
        A::DeleteThread,
        "Space menu",
        "threads: delete the thread here",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('v'), c('s')]],
        A::SourceView,
        "Space menu",
        "view: toggle source view",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('v'), c('d')]],
        A::DiffHead,
        "Space menu",
        "view: toggle the diff against HEAD",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('v'), c('D')]],
        A::DiffSeen,
        "Space menu",
        "view: toggle the diff against last seen",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('j'), c('j')]],
        A::JumpNewest,
        "Space menu",
        "jump to newest change",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('j'), c('a')]],
        A::AutoJumpToggle,
        "Space menu",
        "toggle auto-jump",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('j'), c('c')]],
        A::ClearChanges,
        "Space menu",
        "clear changes",
    ),
    bind(
        W::Any,
        &[&[c(' '), c('w')]],
        A::Wake,
        "Space menu",
        "wake an agent",
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
    bind(
        W::Tree,
        &[&[c('R')]],
        A::TreeRefresh,
        "Tree",
        "re-read directories",
    ),
    bind(
        W::Tree,
        &[&[c('I')]],
        A::TreeIgnored,
        "Tree",
        "show ignored entries",
    ),
    bind(
        W::Tree,
        &[&[k(K::Esc)]],
        A::Escape,
        "Tree",
        "back to the text",
    ),
    // ----- the thread pane -----
    bind(
        W::ThreadPane,
        &[&[c('j')], &[k(K::Down)]],
        A::MoveDown,
        "Thread pane",
        "next message",
    ),
    bind(
        W::ThreadPane,
        &[&[c('k')], &[k(K::Up)]],
        A::MoveUp,
        "Thread pane",
        "previous message",
    ),
    bind(
        W::ThreadPane,
        &[&[c('l')], &[k(K::Right)]],
        A::ThreadNext,
        "Thread pane",
        "next thread in the file",
    ),
    bind(
        W::ThreadPane,
        &[&[c('h')], &[k(K::Left)]],
        A::ThreadPrev,
        "Thread pane",
        "previous thread in the file; on the first, the threads pane",
    ),
    bind(
        W::ThreadPane,
        &[&[c('L')]],
        A::ThreadNextAcross,
        "Thread pane",
        "next thread across the workspace",
    ),
    bind(
        W::ThreadPane,
        &[&[c('H')]],
        A::ThreadPrevAcross,
        "Thread pane",
        "previous thread across the workspace",
    ),
    bind(
        W::ThreadPane,
        &[&[c('g'), c('g')]],
        A::Top,
        "Thread pane",
        "first message",
    ),
    bind(
        W::ThreadPane,
        &[&[c('g'), c('e')], &[c('G')]],
        A::Bottom,
        "Thread pane",
        "last message",
    ),
    bind(
        W::ThreadPane,
        &[&[ctrl('d')]],
        A::HalfPageDown,
        "Thread pane",
        "scroll half the pane down",
    ),
    bind(
        W::ThreadPane,
        &[&[ctrl('u')]],
        A::HalfPageUp,
        "Thread pane",
        "scroll half the pane up",
    ),
    bind(
        W::ThreadPane,
        &[&[c('r')]],
        A::Reply,
        "Thread pane",
        "reply",
    ),
    bind(
        W::ThreadPane,
        &[&[c('e')]],
        A::EditMessage,
        "Thread pane",
        "edit your message",
    ),
    bind(
        W::ThreadPane,
        &[&[c('o')]],
        A::ToggleResolved,
        "Thread pane",
        "resolve or reopen",
    ),
    bind(
        W::ThreadPane,
        &[&[c('d'), c('d')]],
        A::Delete,
        "Thread pane",
        "delete the thread",
    ),
    bind(
        W::ThreadPane,
        &[&[k(K::Esc)]],
        A::Escape,
        "Thread pane",
        "back to the text; the pane stays",
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
        "open the thread pane on the highlight",
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
        A::PaneResolved,
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
    // ----- the thread list -----
    bind(
        W::List,
        &[&[c('j')], &[k(K::Down)]],
        A::MoveDown,
        "Thread list",
        "next message",
    ),
    bind(
        W::List,
        &[&[c('k')], &[k(K::Up)]],
        A::MoveUp,
        "Thread list",
        "previous message",
    ),
    bind(
        W::List,
        &[&[c('l')], &[k(K::Right)]],
        A::ThreadNext,
        "Thread list",
        "next thread",
    ),
    bind(
        W::List,
        &[&[c('h')], &[k(K::Left)]],
        A::ThreadPrev,
        "Thread list",
        "previous thread",
    ),
    bind(
        W::List,
        &[&[c('g'), c('g')]],
        A::Top,
        "Thread list",
        "first thread",
    ),
    bind(
        W::List,
        &[&[c('g'), c('e')], &[c('G')]],
        A::Bottom,
        "Thread list",
        "last thread",
    ),
    bind(
        W::List,
        &[&[ctrl('d')]],
        A::HalfPageDown,
        "Thread list",
        "half a page down",
    ),
    bind(
        W::List,
        &[&[ctrl('u')]],
        A::HalfPageUp,
        "Thread list",
        "half a page up",
    ),
    bind(
        W::List,
        &[&[k(K::Enter)]],
        A::Confirm,
        "Thread list",
        "open the file and thread pane on this message",
    ),
    bind(W::List, &[&[c('r')]], A::Reply, "Thread list", "reply"),
    bind(
        W::List,
        &[&[c('e')]],
        A::EditMessage,
        "Thread list",
        "edit your message",
    ),
    bind(
        W::List,
        &[&[c('o')]],
        A::ToggleResolved,
        "Thread list",
        "resolve or reopen",
    ),
    bind(
        W::List,
        &[&[c('d'), c('d')]],
        A::Delete,
        "Thread list",
        "delete the thread",
    ),
    bind(
        W::List,
        &[&[c('z')]],
        A::Fold,
        "Thread list",
        "fold the entry",
    ),
    bind(
        W::List,
        &[&[c('Z')]],
        A::FoldResolved,
        "Thread list",
        "fold resolved threads",
    ),
    bind(
        W::List,
        &[&[c('f')]],
        A::FileOnly,
        "Thread list",
        "only this file",
    ),
    bind(
        W::List,
        &[&[k(K::Esc)]],
        A::Escape,
        "Thread list",
        "back to the text",
    ),
    // ----- the comment box -----
    bind(
        W::Box,
        &[&[k(K::Enter)]],
        A::Confirm,
        "Comment box",
        "submit, or save",
    ),
    bind(
        W::Box,
        &[
            &[alt(K::Enter)],
            &[Chord {
                key: K::Enter,
                ctrl: true,
                alt: false,
            }],
        ],
        A::Newline,
        "Comment box",
        "newline",
    ),
    bind(
        W::Box,
        &[&[alt(K::Char('k'))], &[alt(K::Up)]],
        A::ScrollUp,
        "Comment box",
        "scroll the thread above up",
    ),
    bind(
        W::Box,
        &[&[alt(K::Char('j'))], &[alt(K::Down)]],
        A::ScrollDown,
        "Comment box",
        "scroll the thread above down",
    ),
    bind(
        W::Box,
        &[&[ctrl('e')]],
        A::EditDraft,
        "Comment box",
        "edit the draft in $EDITOR",
    ),
    bind(W::Box, &[&[k(K::Left)]], A::MoveLeft, "Comment box", "left"),
    bind(
        W::Box,
        &[&[k(K::Right)]],
        A::MoveRight,
        "Comment box",
        "right",
    ),
    bind(W::Box, &[&[k(K::Up)]], A::MoveUp, "Comment box", "up"),
    bind(W::Box, &[&[k(K::Down)]], A::MoveDown, "Comment box", "down"),
    bind(
        W::Box,
        &[&[k(K::Home)], &[ctrl('a')]],
        A::LineStart,
        "Comment box",
        "line start",
    ),
    bind(
        W::Box,
        &[&[k(K::End)]],
        A::LineEnd,
        "Comment box",
        "line end",
    ),
    bind(
        W::Box,
        &[&[alt(K::Char('b'))]],
        A::WordBack,
        "Comment box",
        "word back",
    ),
    bind(
        W::Box,
        &[&[alt(K::Char('f'))]],
        A::WordForward,
        "Comment box",
        "word forward",
    ),
    bind(
        W::Box,
        &[&[k(K::Backspace)]],
        A::Backspace,
        "Comment box",
        "delete back",
    ),
    bind(
        W::Box,
        &[&[k(K::Delete)]],
        A::DeleteForward,
        "Comment box",
        "delete forward",
    ),
    bind(
        W::Box,
        &[&[ctrl('w')]],
        A::DeleteWordBack,
        "Comment box",
        "delete the word back",
    ),
    bind(
        W::Box,
        &[&[ctrl('u')]],
        A::DeleteToLineStart,
        "Comment box",
        "delete to line start",
    ),
    bind(
        W::Box,
        &[&[ctrl('k')]],
        A::DeleteToLineEnd,
        "Comment box",
        "delete to line end",
    ),
    bind(
        W::Box,
        &[&[ctrl('c')]],
        A::ClearDraft,
        "Comment box",
        "clear the draft; empty closes",
    ),
    bind(
        W::Box,
        &[&[k(K::Esc)]],
        A::Escape,
        "Comment box",
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
/// breadcrumb row name them by (ADR 0049).
const SUBMENUS: &[(Keys, &str)] = &[
    (&[c(' '), c('r')], "rail"),
    (&[c(' '), c('c')], "threads"),
    (&[c(' '), c('v')], "view"),
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
/// spelled, then the submenu's word (`Space c · threads`).
#[must_use]
pub fn menu_title(typed: &[Chord]) -> String {
    match submenu_word(typed) {
        Some(word) => format!("{} · {word}", spell(typed)),
        None => spell(typed),
    }
}

/// `Ctrl` letters zellij's lock mode owns; the viewer never binds them.
#[cfg(test)]
pub const ZELLIJ_LOCKS: [char; 9] = ['g', 'p', 't', 'n', 'h', 's', 'o', 'q', 'b'];

/// What a typed sequence is on one surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
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
pub fn lookup(place: Where, typed: &[Chord]) -> Match {
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

/// The which-key entries for `typed` on `place`: the next key of every
/// binding that continues it, with its label, in table order.
#[must_use]
pub fn menu(place: Where, typed: &[Chord]) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for binding in applicable(place) {
        for keys in binding.keys {
            if keys.len() > typed.len() && keys.starts_with(typed) {
                let next = keys[typed.len()].to_string();
                if !entries.iter().any(|(key, _)| *key == next) {
                    let label = if keys.len() == typed.len() + 1 {
                        binding.label.to_owned()
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
pub fn hint(place: Where, action: Action) -> Option<String> {
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
    own.or_else(any)
        .and_then(|b| b.keys.first())
        .map(|keys| spell(keys))
}

/// The help popup: one row per binding, keys joined by ` / `, under a
/// header row per group.
#[must_use]
pub fn help() -> Vec<(String, String)> {
    let mut rows = Vec::with_capacity(BINDINGS.len() + 16);
    let mut group = "";
    for binding in BINDINGS {
        if binding.group != group {
            group = binding.group;
            rows.push((String::new(), group.to_owned()));
        }
        let keys = binding
            .keys
            .iter()
            .map(|keys| spell(keys))
            .collect::<Vec<_>>()
            .join(" / ");
        rows.push((keys, binding.label.to_owned()));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{
        Action, BINDINGS, Chord, Key, Match, Where, ZELLIJ_LOCKS, c, help, hint, k, lookup, menu,
        spell,
    };

    const PANES: [Where; 5] = [
        Where::View,
        Where::Tree,
        Where::ThreadPane,
        Where::ThreadsPane,
        Where::List,
    ];

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
            Where::ThreadPane,
            Where::ThreadsPane,
            Where::List,
            Where::Box,
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
        for place in [Where::Box, Where::Picker, Where::Input] {
            assert_eq!(lookup(place, &[k(Key::Esc)]), Match::Exact(Action::Escape));
            assert_eq!(lookup(place, &[c(' ')]), Match::Miss, "{place:?}");
        }
    }

    /// The menu after `Space` lists each entry once with its next key,
    /// and the submenus open under `j`, `c`, `v`, and `r` (ADR 0049).
    #[test]
    fn menus_come_from_the_table() {
        let space = menu(Where::View, &[c(' ')]);
        assert!(
            space
                .iter()
                .any(|(key, label)| key == "?" && label == "all keys")
        );
        for (key, word) in [
            ("j", "jump…"),
            ("c", "threads…"),
            ("v", "view…"),
            ("r", "rail…"),
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
        assert_eq!(keys(Where::Tree, &[c(' '), c('j')]), ["j", "a", "c"]);
        assert_eq!(
            keys(Where::View, &[c(' '), c('c')]),
            ["c", "x", "n", "r", "o", "e", "d"]
        );
        assert_eq!(keys(Where::List, &[c(' '), c('v')]), ["s", "d", "D"]);
        assert_eq!(keys(Where::View, &[c(' '), c('r')]), ["r", "i", "."]);
        assert!(menu(Where::Box, &[c(' ')]).is_empty());
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
        let guide = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/guide.md");
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
        assert_eq!(hint(Where::List, Action::Reply).as_deref(), Some("r"));
        assert_eq!(hint(Where::Tree, Action::TreeRefresh).as_deref(), Some("R"));
        assert_eq!(
            hint(Where::View, Action::Reply).as_deref(),
            Some("Space c r")
        );
        assert_eq!(hint(Where::List, Action::CommandLine).as_deref(), Some(":"));
        assert_eq!(hint(Where::Box, Action::CommandLine), None);
        assert!(help().iter().any(|(keys, _)| keys == "j / Down"));
    }
}
