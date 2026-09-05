// @okf-doc: /decisions/0053-resolution-is-the-users.md
//! Threads an agent proposes resolving (ADR 0053).
//!
//! An agent's `resolve` on a reply only proposes: the thread stays open
//! and waiting ([`Thread::proposes_resolution`]), its rows read
//! `proposed` after the state word, and the user's `o` closes it. This
//! module counts the proposals for the status line, the review list's
//! header, and `:status`; the waiting machinery (ADR 0030) does the rest.

use fathomable_core::annotations::{Store, Thread};

use crate::app::App;

impl App {
    /// Proposed threads on the current document.
    pub(crate) fn proposed_count(&self) -> usize {
        self.marks()
            .iter()
            .filter_map(|mark| self.thread(mark.id()))
            .filter(|thread| thread.proposes_resolution())
            .count()
    }

    /// Proposed threads across the work in scope.
    pub(crate) fn proposed_total(&self) -> usize {
        self.store
            .iter()
            .flat_map(Store::threads)
            .filter(|thread| self.reach.includes(thread) && Thread::proposes_resolution(thread))
            .count()
    }
}
