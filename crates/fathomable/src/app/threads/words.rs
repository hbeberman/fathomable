// @okf-doc: /decisions/0032-placement-and-state.md
//! The two words that describe a thread (ADR 0032).
//!
//! A [`ThreadState`] is the *state* and the one colour a thread has (ADR
//! 0039). The thread pane and the file-threads pane say more: the
//! *placement* word (`detached`, `edited`) when the lines moved or went,
//! then the state word (`waiting`, `open`, `resolved`, `auto-resolved`)
//! always, so where a thread's lines are never hides what it needs.

use fathomable_core::annotations::{Placement, Thread};

use crate::app::threads::ThreadState;

/// The placement word, when the lines are not where the comment was
/// written, and the state word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Words {
    placement: Option<&'static str>,
    state: ThreadState,
}

impl Words {
    /// Words for `thread` at `placement`, known when its file is open.
    #[must_use]
    pub fn of(placement: Option<Placement>, thread: &Thread) -> Self {
        let placement = placement.and_then(|placement| match placement {
            Placement::Detached(_) => Some("detached"),
            Placement::Edited(_) => Some("edited"),
            Placement::Anchored(_) => None,
        });
        Self {
            placement,
            state: ThreadState::of(thread),
        }
    }

    /// `detached` or `edited`, when the lines are not where they were.
    #[must_use]
    pub fn placement(self) -> Option<&'static str> {
        self.placement
    }

    /// The state word's kind: waiting, open, resolved, or auto-resolved.
    #[must_use]
    pub fn state(self) -> ThreadState {
        self.state
    }

    /// Whether the thread is resolved, however it is placed.
    #[must_use]
    pub fn is_resolved(self) -> bool {
        matches!(
            self.state,
            ThreadState::Resolved | ThreadState::AutoResolved
        )
    }
}

/// The status word for a kind.
#[must_use]
pub fn label(kind: ThreadState) -> &'static str {
    match kind {
        ThreadState::Waiting => "waiting",
        ThreadState::Open => "open",
        ThreadState::Resolved => "resolved",
        ThreadState::AutoResolved => "auto-resolved",
    }
}
