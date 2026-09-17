// @okf-doc: /decisions/0016-syntax-highlighting.md
//! Whole-file source highlighting away from the terminal event loop.
//!
//! The app queues immutable text generations. One worker colours them in
//! sequence and drops queued intermediate previews before starting the next
//! file, so rapidly paging the files pane cannot create a CPU backlog.

use std::io;
use std::sync::{Arc, mpsc as std_mpsc};
use std::thread;

use fathomable_core::highlight::{Highlighter, Highlights};
use tokio::sync::mpsc;

use super::App;

/// Pending work and stable identities for source documents.
#[derive(Debug)]
pub(crate) struct Queue {
    next_document: u64,
    jobs: Vec<Job>,
}

impl Queue {
    pub(crate) const fn new() -> Self {
        Self {
            next_document: 0,
            jobs: Vec::new(),
        }
    }

    pub(crate) fn next_document(&mut self) -> u64 {
        let document = self.next_document;
        let Some(next) = document.checked_add(1) else {
            unreachable!("source document identity overflow");
        };
        self.next_document = next;
        document
    }

    pub(crate) fn clear(&mut self) {
        self.jobs.clear();
    }
}

/// One immutable document generation to highlight.
#[derive(Debug)]
pub(crate) struct Job {
    document: u64,
    generation: u64,
    text: String,
    hint: String,
    highlighter: Arc<Highlighter>,
}

impl Job {
    fn key(&self) -> (u64, u64) {
        (self.document, self.generation)
    }

    fn run(self) -> Highlighted {
        let runs = self.highlighter.highlight(&self.text, &self.hint);
        Highlighted {
            document: self.document,
            generation: self.generation,
            runs,
        }
    }
}

/// Highlighting returned by the worker for one document generation.
#[derive(Debug)]
pub(crate) struct Highlighted {
    document: u64,
    generation: u64,
    runs: Option<Highlights>,
}

/// The request thread and its asynchronous result channel.
#[derive(Debug)]
pub(crate) struct Worker {
    jobs: std_mpsc::Sender<Job>,
    completed: mpsc::UnboundedReceiver<Highlighted>,
    _thread: thread::JoinHandle<()>,
}

impl Worker {
    /// Start the one source-highlighting thread.
    pub(crate) fn new() -> io::Result<Self> {
        let (job_tx, job_rx) = std_mpsc::channel::<Job>();
        let (completed_tx, completed_rx) = mpsc::unbounded_channel();
        let thread = thread::Builder::new()
            .name("syntax highlight".to_owned())
            .spawn(move || {
                let mut completed_key = None;
                while let Ok(mut job) = job_rx.recv() {
                    while let Ok(newer) = job_rx.try_recv() {
                        job = newer;
                    }
                    if completed_key == Some(job.key()) {
                        continue;
                    }
                    let highlighted = job.run();
                    completed_key = Some((highlighted.document, highlighted.generation));
                    if completed_tx.send(highlighted).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            jobs: job_tx,
            completed: completed_rx,
            _thread: thread,
        })
    }

    /// Submit the latest requests accumulated by the app.
    pub(crate) fn submit(&self, jobs: Vec<Job>) -> Result<(), std_mpsc::SendError<Job>> {
        for job in jobs {
            self.jobs.send(job)?;
        }
        Ok(())
    }

    /// Wait for the next completed source generation.
    pub(crate) async fn next(&mut self) -> Option<Highlighted> {
        self.completed.recv().await
    }
}

impl App {
    /// Queue the current source generation, replacing older queued previews.
    pub(super) fn queue_highlight(&mut self, index: usize) {
        if self.current != Some(index) {
            return;
        }
        let Some(doc) = self.docs.get(index) else {
            return;
        };
        let Some((generation, text, hint, highlighter)) = doc.view.pending_highlight() else {
            return;
        };
        let document = doc.id;
        self.highlights.jobs.clear();
        self.highlights.jobs.push(Job {
            document,
            generation,
            text,
            hint,
            highlighter,
        });
    }

    /// Drain source generations waiting to be sent to the worker.
    pub(crate) fn take_highlight_jobs(&mut self) -> Vec<Job> {
        std::mem::take(&mut self.highlights.jobs)
    }

    /// Apply a completed generation, rejecting removed or reloaded documents.
    pub(crate) fn apply_highlight(&mut self, highlighted: Highlighted) -> bool {
        let Some((index, doc)) = self
            .docs
            .iter_mut()
            .enumerate()
            .find(|(_, doc)| doc.id == highlighted.document)
        else {
            return false;
        };
        let changed = doc
            .view
            .apply_highlight(highlighted.generation, highlighted.runs);
        changed && self.current == Some(index)
    }
}

#[cfg(test)]
impl Job {
    pub(crate) fn complete(self) -> Highlighted {
        self.run()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use anyhow::Context as _;
    use fathomable_core::highlight::Highlighter;

    use super::{Job, Worker};

    #[tokio::test(flavor = "current_thread")]
    async fn worker_returns_a_completed_generation() -> anyhow::Result<()> {
        let mut worker = Worker::new()?;
        worker.submit(vec![Job {
            document: 7,
            generation: 3,
            text: "fn main() {}\n".to_owned(),
            hint: "rs".to_owned(),
            highlighter: Arc::new(Highlighter::new("base16-ocean.dark")?),
        }])?;
        let highlighted = tokio::time::timeout(Duration::from_secs(2), worker.next())
            .await?
            .context("worker stopped")?;
        assert_eq!((highlighted.document, highlighted.generation), (7, 3));
        assert!(highlighted.runs.is_some());
        Ok(())
    }
}
