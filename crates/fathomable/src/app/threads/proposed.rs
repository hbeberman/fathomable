// @okf-doc: /decisions/0053-resolution-is-the-users.md
//! Threads an agent proposes resolving (ADR 0053).
//!
//! An unauthorized resolving agent reply leaves the thread in the durable
//! resolution-proposed lifecycle. This module counts those threads for the
//! status line and `:status`.

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
            .filter(|thread| self.normal_thread(thread) && Thread::proposes_resolution(thread))
            .count()
    }
}
