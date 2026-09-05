// @okf-doc: /decisions/0032-placement-and-state.md
//! The words that describe a thread (ADR 0032, ADR 0053).
//!
//! A [`ThreadState`] is the *state* and the one colour a thread has (ADR
//! 0039). The expanded thread's header and the review list say more: the
//! *placement* word (`detached`, `edited`) when the lines moved or went,
//! then the state word (`waiting`, `open`, `resolved`) always, then
//! `proposed` when an agent's newest reply proposes resolving, so where
//! a thread's lines are never hides what it needs.

use fathomable_core::annotations::{Placement, Thread};

use crate::app::threads::ThreadState;

/// The placement word, when the lines are not where the comment was
/// written, the state word, and whether `proposed` follows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Words {
    placement: Option<&'static str>,
    state: ThreadState,
    proposed: bool,
}

impl Words {
    /// Words for `thread` at `placement`, known when its file is open.
    #[must_use]
    pub(crate) fn of(placement: Option<Placement>, thread: &Thread) -> Self {
        let placement = placement.and_then(|placement| match placement {
            Placement::Detached(_) => Some("detached"),
            Placement::Edited(_) => Some("edited"),
            Placement::Anchored(_) => None,
        });
        Self {
            placement,
            state: ThreadState::of(thread),
            proposed: thread.proposes_resolution(),
        }
    }

    /// `detached` or `edited`, when the lines are not where they were.
    #[must_use]
    pub(crate) fn placement(self) -> Option<&'static str> {
        self.placement
    }

    /// The state word's kind: waiting, open, or resolved.
    #[must_use]
    pub(crate) fn state(self) -> ThreadState {
        self.state
    }

    /// Whether `proposed` follows the state word: the thread is open and
    /// an agent's newest reply proposes resolving it (ADR 0053).
    #[must_use]
    pub(crate) fn proposed(self) -> bool {
        self.proposed
    }

    /// Whether the thread is resolved, however it is placed.
    #[must_use]
    pub(crate) fn is_resolved(self) -> bool {
        self.state == ThreadState::Resolved
    }
}

/// The status word for a kind.
#[must_use]
pub(crate) fn label(kind: ThreadState) -> &'static str {
    match kind {
        ThreadState::Waiting => "waiting",
        ThreadState::Open => "open",
        ThreadState::Resolved => "resolved",
    }
}
