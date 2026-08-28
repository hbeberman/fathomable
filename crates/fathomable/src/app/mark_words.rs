// @okf-doc: /decisions/0032-placement-and-state.md
//! The two words that describe a thread (ADR 0032).
//!
//! A [`MarkKind`] ranks one colour for the gutter, and placement wins
//! there: a detached thread is red whatever its state. The thread pane
//! and the file-threads pane say more: the *placement* word (`detached`,
//! `edited`) when the lines moved or went, then the *state* word
//! (`waiting`, `open`, `resolved`, `auto-resolved`) always, so a reply or
//! a resolution on a detached thread is never hidden behind its colour.

use fathomable_core::annotations::{Status, Thread};

use super::threads::MarkKind;

/// The placement word, when the lines are not where the comment was
/// written, and the state word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Words {
    placement: Option<MarkKind>,
    state: MarkKind,
}

impl Words {
    /// Words for `thread` shown with `kind`, its gutter kind when the
    /// file is open.
    #[must_use]
    pub fn of(kind: Option<MarkKind>, thread: &Thread) -> Self {
        let placement = kind.filter(|kind| matches!(kind, MarkKind::Detached | MarkKind::Edited));
        let state = if thread.awaits_user() {
            MarkKind::Waiting
        } else {
            match thread.status() {
                Status::Open => MarkKind::Open,
                Status::Resolved => MarkKind::Resolved,
                Status::AutoResolved => MarkKind::AutoResolved,
            }
        };
        Self { placement, state }
    }

    /// `detached` or `edited`, when the lines are not where they were.
    #[must_use]
    pub fn placement(self) -> Option<MarkKind> {
        self.placement
    }

    /// The state word's kind: waiting, open, resolved, or auto-resolved.
    #[must_use]
    pub fn state(self) -> MarkKind {
        self.state
    }

    /// Whether the thread is resolved, however it is placed.
    #[must_use]
    pub fn is_resolved(self) -> bool {
        matches!(self.state, MarkKind::Resolved | MarkKind::AutoResolved)
    }
}

/// The status word for a kind.
#[must_use]
pub fn label(kind: MarkKind) -> &'static str {
    match kind {
        MarkKind::Detached => "detached",
        MarkKind::Edited => "edited",
        MarkKind::Waiting => "waiting",
        MarkKind::Open => "open",
        MarkKind::Resolved => "resolved",
        MarkKind::AutoResolved => "auto-resolved",
    }
}
