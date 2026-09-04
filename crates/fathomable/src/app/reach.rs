// @okf-doc: /decisions/0035-threads-follow-head.md
//! Threads that follow `HEAD` across a history rewrite (ADR 0035).
//!
//! A thread belongs to the commit it was written on and shows while that
//! commit is reachable from `HEAD` (ADR 0024). An amend, squash, or rebase
//! takes the commit out of history although the lines are still in the
//! working tree; such a thread, while open, is moved to the new `HEAD`
//! instead of vanishing.

use std::collections::HashSet;
use std::fs;

use fathomable_core::annotations::{Status, Store, ThreadId};
use fathomable_core::workspace::Workspace;

use super::threads::now;

/// Rescope to `HEAD` every open thread whose commit `HEAD` cannot reach
/// but whose lines are still in its file on disk. `reachable` is the
/// answer of [`Workspace::reachable`] for the store's commits. Returns
/// how many threads moved.
pub(crate) fn follow_head(
    store: &mut Store,
    workspace: &Workspace,
    reachable: &HashSet<String>,
) -> usize {
    let Some(head) = workspace.head_commit() else {
        return 0;
    };
    let stranded: Vec<ThreadId> = store
        .threads()
        .iter()
        .filter(|thread| thread.status() == Status::Open)
        .filter(|thread| {
            thread
                .commit()
                .is_some_and(|commit| commit != head && !reachable.contains(commit))
        })
        .filter(|thread| {
            fs::read_to_string(workspace.root().join(thread.path()))
                .is_ok_and(|text| !thread.locate(&text).is_detached())
        })
        .map(|thread| thread.id().clone())
        .collect();
    let mut moved = 0;
    for id in stranded {
        match store.rescope(&id, &head, now()) {
            Ok(()) => {
                moved += 1;
                tracing::info!(%id, %head, "thread rescoped to the new HEAD");
            }
            Err(error) => tracing::warn!(%id, %error, "cannot rescope thread"),
        }
    }
    moved
}
