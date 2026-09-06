//! The dirty set's full walk, off the event loop (ADR 0017).
//!
//! A walk of the whole tree costs one `stat` per tracked file and one
//! `readdir` per directory, a quarter of a second on sixty thousand
//! files, so it runs on a thread of its own and the loop takes its
//! result when it lands. Until then the set stays what it was, empty at
//! start, and the paths that change meanwhile are replayed onto the
//! result, so a write during the walk is never lost. A newer walk
//! supersedes an older one still running.

use std::io;
use std::path::PathBuf;
use std::thread;

use fathomable_core::status::Status;
use fathomable_core::workspace::{Workspace, WorkspaceError};
use tokio::sync::mpsc;

/// What a walk thread sends back: which walk, and what it found.
#[derive(Debug)]
pub(crate) struct Walked {
    seq: u64,
    result: Result<Status, WorkspaceError>,
}

/// A walk in flight, and the root-relative paths that changed since it
/// started.
#[derive(Debug)]
struct Pending {
    seq: u64,
    changed: Vec<PathBuf>,
}

/// The walk threads and the channel they answer on.
#[derive(Debug)]
pub(super) struct Walks {
    tx: mpsc::UnboundedSender<Walked>,
    rx: mpsc::UnboundedReceiver<Walked>,
    seq: u64,
    pending: Option<Pending>,
}

impl Walks {
    pub(super) fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx,
            seq: 0,
            pending: None,
        }
    }

    /// Walk the repository at `root` on a thread of its own; its result
    /// comes back through [`Walks::next`]. A walk already running is
    /// left to finish and its result dropped.
    ///
    /// # Errors
    ///
    /// Returns the error when the thread cannot be started.
    pub(super) fn start(&mut self, root: PathBuf) -> io::Result<()> {
        self.seq += 1;
        let seq = self.seq;
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("git status".to_owned())
            .spawn(move || {
                let result =
                    Workspace::discover(&root).and_then(|mut workspace| workspace.status());
                // The app is gone when nobody listens; nothing to report.
                let _ = tx.send(Walked { seq, result });
            })?;
        self.pending = Some(Pending {
            seq,
            changed: Vec::new(),
        });
        tracing::debug!(seq, "git status walk started");
        Ok(())
    }

    /// Whether a walk is running.
    pub(super) fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    /// Root-relative `changed` moved while the walk runs: they are
    /// examined again on its result.
    pub(super) fn note_changed(&mut self, changed: &[PathBuf]) {
        if let Some(pending) = self.pending.as_mut() {
            pending.changed.extend(changed.iter().cloned());
        }
    }

    /// The next result a walk thread sends.
    pub(super) async fn next(&mut self) -> Walked {
        match self.rx.recv().await {
            Some(walked) => walked,
            // The sender is ours, so the channel never closes.
            None => std::future::pending().await,
        }
    }

    /// The next result, waiting for it on this thread.
    #[cfg(test)]
    pub(super) fn blocking_next(&mut self) -> Option<Walked> {
        self.rx.blocking_recv()
    }

    /// Take `walked` if it is the walk in flight: its result and the
    /// paths that changed since it started. `None` for an older walk a
    /// newer one replaced.
    pub(super) fn accept(
        &mut self,
        walked: Walked,
    ) -> Option<(Result<Status, WorkspaceError>, Vec<PathBuf>)> {
        let pending = self.pending.as_ref()?;
        if pending.seq != walked.seq {
            tracing::debug!(seq = walked.seq, "git status walk superseded");
            return None;
        }
        let pending = self.pending.take()?;
        Some((walked.result, pending.changed))
    }
}
