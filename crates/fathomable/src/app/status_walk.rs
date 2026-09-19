// @okf-doc: /decisions/0017-git-status-navigation.md
//! Git status discovery outside the event loop.
//!
//! One persistent thread executes at most one request while retaining at most
//! one replacement request and one undelivered result. New requests cancel the
//! executing request. Incremental requests coalesce their paths until the
//! configured event or retained-path budget is exhausted, then become a full
//! rescan instead of losing paths.

#![expect(
    clippy::expect_used,
    reason = "poisoned synchronization and failed test setup are programming errors"
)]

use std::collections::BTreeSet;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use fathomable_core::config::LimitsConfig;
use fathomable_core::status::Status;
use fathomable_core::workspace::{Cancellation, Workspace, WorkspaceError};
use tokio::sync::mpsc;

type StatusResult = Result<Status, WorkspaceError>;
type Executor = dyn Fn(Input, Cancellation) -> StatusResult + Send + Sync;

/// What the status worker sends back.
#[derive(Debug)]
pub(crate) struct Walked {
    generation: u64,
    root: PathBuf,
    result: StatusResult,
}

#[derive(Debug, Clone)]
enum Work {
    Full,
    Incremental {
        previous: Arc<Status>,
        changed: BTreeSet<PathBuf>,
        event_count: usize,
    },
}

#[derive(Debug, Clone)]
struct Input {
    root: PathBuf,
    limits: LimitsConfig,
    work: Work,
}

#[derive(Debug)]
struct Request {
    generation: u64,
    cancellation: Cancellation,
    input: Input,
}

#[derive(Debug)]
struct Mailbox {
    request: Option<Request>,
    stopped: bool,
}

#[derive(Debug, Clone)]
struct Pending {
    generation: u64,
    input: Input,
}

/// One status thread with bounded request and result mailboxes.
pub(super) struct Walks {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
    receiver: Option<mpsc::Receiver<Walked>>,
    executor: Arc<Executor>,
    cancellation: Cancellation,
    generation: u64,
    pending: Option<Pending>,
}

impl fmt::Debug for Walks {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Walks")
            .field("generation", &self.generation)
            .field("pending", &self.pending)
            .finish_non_exhaustive()
    }
}

impl Walks {
    pub(super) fn new() -> Self {
        Self::with_executor(Arc::new(execute))
    }

    fn with_executor(executor: Arc<Executor>) -> Self {
        Self {
            mailbox: Arc::new((
                Mutex::new(Mailbox {
                    request: None,
                    stopped: false,
                }),
                Condvar::new(),
            )),
            receiver: None,
            executor,
            cancellation: Cancellation::default(),
            generation: 0,
            pending: None,
        }
    }

    /// Request a full status scan.
    ///
    /// A request already executing is cooperatively cancelled and the one
    /// pending replacement is overwritten.
    ///
    /// # Errors
    ///
    /// Returns the error when the persistent worker thread cannot be started.
    pub(super) fn start(&mut self, root: PathBuf, limits: LimitsConfig) -> io::Result<()> {
        self.submit(Input {
            root,
            limits,
            work: Work::Full,
        })
    }

    /// Request status after a bounded set of root-relative changes.
    ///
    /// Paths from a superseded incremental request are retained. If either
    /// the event count or distinct-path count exceeds its configured limit,
    /// the replacement is a full scan.
    ///
    /// # Errors
    ///
    /// Returns the error when the persistent worker thread cannot be started.
    pub(super) fn incremental(
        &mut self,
        root: PathBuf,
        limits: LimitsConfig,
        previous: Status,
        changed: &[PathBuf],
    ) -> io::Result<()> {
        let work = self.coalesced_work(&root, &limits, previous, changed);
        self.submit(Input { root, limits, work })
    }

    fn coalesced_work(
        &self,
        root: &Path,
        limits: &LimitsConfig,
        previous: Status,
        changed: &[PathBuf],
    ) -> Work {
        let (previous, mut paths, event_count) = match &self.pending {
            Some(Pending {
                input:
                    Input {
                        root: pending_root,
                        work:
                            Work::Incremental {
                                previous,
                                changed: pending_changed,
                                event_count: pending_event_count,
                            },
                        ..
                    },
                ..
            }) if pending_root == root => {
                let event_count = pending_event_count.saturating_add(changed.len());
                if event_count > limits.pending_events
                    || pending_changed.len() > limits.retained_paths
                {
                    return Work::Full;
                }
                (Arc::clone(previous), pending_changed.clone(), event_count)
            }
            Some(Pending {
                input:
                    Input {
                        root: pending_root,
                        work: Work::Full,
                        ..
                    },
                ..
            }) if pending_root == root => return Work::Full,
            _ => {
                if changed.len() > limits.pending_events {
                    return Work::Full;
                }
                (Arc::new(previous), BTreeSet::new(), changed.len())
            }
        };
        for path in changed {
            paths.insert(path.clone());
            if paths.len() > limits.retained_paths {
                return Work::Full;
            }
        }
        Work::Incremental {
            previous,
            changed: paths,
            event_count,
        }
    }

    fn submit(&mut self, input: Input) -> io::Result<()> {
        self.ensure_worker()?;
        self.cancellation.cancel();
        self.generation = self.generation.wrapping_add(1);
        self.cancellation = Cancellation::default();
        self.pending = Some(Pending {
            generation: self.generation,
            input: input.clone(),
        });
        let (lock, wake) = &*self.mailbox;
        lock.lock().expect("status worker mailbox poisoned").request = Some(Request {
            generation: self.generation,
            cancellation: self.cancellation.clone(),
            input,
        });
        wake.notify_one();
        Ok(())
    }

    fn ensure_worker(&mut self) -> io::Result<()> {
        if self.receiver.is_some() {
            return Ok(());
        }
        let (sender, receiver) = mpsc::channel(1);
        let mailbox = Arc::clone(&self.mailbox);
        let executor = Arc::clone(&self.executor);
        thread::Builder::new()
            .name("git status".to_owned())
            .spawn(move || worker(&mailbox, executor.as_ref(), &sender))?;
        self.receiver = Some(receiver);
        Ok(())
    }

    /// Whether a current status request has not yet been accepted.
    #[cfg(test)]
    pub(super) fn in_flight(&self) -> bool {
        self.pending.is_some()
    }

    /// Wait for the next result sent by the status thread.
    pub(super) async fn next(&mut self) -> Walked {
        match self.receiver.as_mut() {
            Some(receiver) => match receiver.recv().await {
                Some(walked) => walked,
                None => std::future::pending().await,
            },
            None => std::future::pending().await,
        }
    }

    /// Wait for the next result on this thread.
    #[cfg(test)]
    pub(super) fn blocking_next(&mut self) -> Option<Walked> {
        self.receiver.as_mut()?.blocking_recv()
    }

    /// Accept a result only for the current generation and workspace root.
    pub(super) fn accept(&mut self, walked: Walked) -> Option<StatusResult> {
        let pending = self.pending.as_ref()?;
        if pending.generation != walked.generation || pending.input.root != walked.root {
            tracing::debug!(
                generation = walked.generation,
                root = %walked.root.display(),
                "git status result superseded"
            );
            return None;
        }
        self.pending = None;
        Some(walked.result)
    }
}

impl Drop for Walks {
    fn drop(&mut self) {
        self.cancellation.cancel();
        let (lock, wake) = &*self.mailbox;
        let mut state = lock.lock().expect("status worker mailbox poisoned");
        state.stopped = true;
        state.request = None;
        wake.notify_one();
    }
}

fn worker(mailbox: &(Mutex<Mailbox>, Condvar), executor: &Executor, sender: &mpsc::Sender<Walked>) {
    loop {
        let request = {
            let (lock, wake) = mailbox;
            let mut state = lock.lock().expect("status worker mailbox poisoned");
            while state.request.is_none() && !state.stopped {
                state = wake.wait(state).expect("status worker mailbox poisoned");
            }
            if state.stopped {
                return;
            }
            state.request.take().expect("notified worker has a request")
        };
        let root = request.input.root.clone();
        let result = executor(request.input, request.cancellation.clone());
        if request.cancellation.is_cancelled() {
            continue;
        }
        if sender
            .blocking_send(Walked {
                generation: request.generation,
                root,
                result,
            })
            .is_err()
        {
            return;
        }
    }
}

fn execute(input: Input, cancellation: Cancellation) -> StatusResult {
    let mut workspace = Workspace::discover(&input.root)?;
    workspace.set_limits(input.limits);
    workspace.set_cancellation(cancellation);
    match input.work {
        Work::Full => workspace.status(),
        Work::Incremental {
            previous, changed, ..
        } => {
            let changed: Vec<_> = changed.into_iter().collect();
            workspace.status_after(&previous, &changed)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc as std_mpsc;
    use std::time::Duration;

    use super::*;

    #[derive(Debug)]
    struct Call {
        thread: thread::ThreadId,
        work: Work,
    }

    #[derive(Debug)]
    struct Calls {
        calls: Mutex<Vec<Call>>,
        wake: Condvar,
    }

    impl Calls {
        fn new() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                wake: Condvar::new(),
            }
        }

        fn record(&self, work: Work) {
            let mut calls = self.calls.lock().expect("call record poisoned");
            calls.push(Call {
                thread: thread::current().id(),
                work,
            });
            self.wake.notify_all();
        }

        fn wait_for(&self, count: usize) {
            let calls = self.calls.lock().expect("call record poisoned");
            let (calls, timeout) = self
                .wake
                .wait_timeout_while(calls, Duration::from_secs(1), |calls| calls.len() < count)
                .expect("call record poisoned");
            assert!(!timeout.timed_out(), "worker did not execute request");
            assert!(calls.len() >= count);
        }
    }

    fn accepted(walks: &mut Walks) -> StatusResult {
        loop {
            let walked = walks.blocking_next().expect("status worker stopped");
            if let Some(result) = walks.accept(walked) {
                return result;
            }
        }
    }

    #[test]
    fn incremental_requests_coalesce_on_the_worker() {
        let calls = Arc::new(Calls::new());
        let invocations = Arc::new(AtomicUsize::new(0));
        let executor = {
            let calls = Arc::clone(&calls);
            let invocations = Arc::clone(&invocations);
            Arc::new(move |input: Input, cancellation: Cancellation| {
                let invocation = invocations.fetch_add(1, Ordering::SeqCst);
                calls.record(input.work);
                if invocation == 0 {
                    while !cancellation.is_cancelled() {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
                Ok(Status::default())
            }) as Arc<Executor>
        };
        let mut walks = Walks::with_executor(executor);
        let limits = LimitsConfig::default();
        let root = PathBuf::from("/workspace");

        walks
            .incremental(
                root.clone(),
                limits.clone(),
                Status::default(),
                &[PathBuf::from("a")],
            )
            .expect("start first request");
        calls.wait_for(1);
        walks
            .incremental(
                root.clone(),
                limits.clone(),
                Status::default(),
                &[PathBuf::from("b")],
            )
            .expect("replace first request");
        walks
            .incremental(
                root,
                limits,
                Status::default(),
                &[PathBuf::from("a"), PathBuf::from("c")],
            )
            .expect("coalesce replacement");

        accepted(&mut walks).expect("latest request succeeds");
        let calls = calls.calls.lock().expect("call record poisoned");
        let Some(Work::Incremental {
            changed,
            event_count,
            ..
        }) = calls.last().map(|call| &call.work)
        else {
            assert!(
                matches!(
                    calls.last().map(|call| &call.work),
                    Some(Work::Incremental { .. })
                ),
                "latest request unexpectedly became a full scan"
            );
            return;
        };
        assert_eq!(
            changed,
            &BTreeSet::from([PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")])
        );
        assert_eq!(*event_count, 4);
        assert!(
            calls.iter().all(|call| call.thread == calls[0].thread),
            "requests ran on more than one worker thread"
        );
        assert_ne!(calls[0].thread, thread::current().id());
    }

    #[test]
    fn coalescing_overflow_becomes_full_scan() {
        let calls = Arc::new(Calls::new());
        let executor = {
            let calls = Arc::clone(&calls);
            Arc::new(move |input: Input, _cancellation: Cancellation| {
                calls.record(input.work);
                Ok(Status::default())
            }) as Arc<Executor>
        };
        let mut walks = Walks::with_executor(executor);
        let limits = LimitsConfig {
            pending_events: 2,
            retained_paths: 2,
            ..LimitsConfig::default()
        };

        walks
            .incremental(
                PathBuf::from("/workspace"),
                limits,
                Status::default(),
                &[PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")],
            )
            .expect("start request");
        accepted(&mut walks).expect("request succeeds");

        let calls = calls.calls.lock().expect("call record poisoned");
        assert!(matches!(
            calls.as_slice(),
            [Call {
                work: Work::Full,
                ..
            }]
        ));
    }

    #[test]
    fn distinct_path_overflow_becomes_full_scan() {
        let walks = Walks::new();
        let limits = LimitsConfig {
            pending_events: 10,
            retained_paths: 2,
            ..LimitsConfig::default()
        };

        let work = walks.coalesced_work(
            Path::new("/workspace"),
            &limits,
            Status::default(),
            &[PathBuf::from("a"), PathBuf::from("b"), PathBuf::from("c")],
        );

        assert!(matches!(work, Work::Full));
    }

    #[test]
    fn accept_rejects_wrong_generation_and_root() {
        let mut walks = Walks::new();
        let input = Input {
            root: PathBuf::from("/current"),
            limits: LimitsConfig::default(),
            work: Work::Full,
        };
        walks.pending = Some(Pending {
            generation: 7,
            input,
        });

        assert!(
            walks
                .accept(Walked {
                    generation: 6,
                    root: PathBuf::from("/current"),
                    result: Ok(Status::default()),
                })
                .is_none()
        );
        assert!(
            walks
                .accept(Walked {
                    generation: 7,
                    root: PathBuf::from("/other"),
                    result: Ok(Status::default()),
                })
                .is_none()
        );
        assert!(
            walks
                .accept(Walked {
                    generation: 7,
                    root: PathBuf::from("/current"),
                    result: Ok(Status::default()),
                })
                .is_some()
        );
    }

    #[test]
    fn drop_does_not_join_an_uncooperative_worker() {
        let started = Arc::new(Calls::new());
        let release = Arc::new(AtomicBool::new(false));
        let executor = {
            let started = Arc::clone(&started);
            let release = Arc::clone(&release);
            Arc::new(move |_input: Input, _cancellation: Cancellation| {
                started.record(Work::Full);
                while !release.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(1));
                }
                Ok(Status::default())
            }) as Arc<Executor>
        };
        let mut walks = Walks::with_executor(executor);
        walks
            .start(PathBuf::from("/workspace"), LimitsConfig::default())
            .expect("start request");
        started.wait_for(1);

        let (dropped_tx, dropped_rx) = std_mpsc::channel();
        thread::spawn(move || {
            drop(walks);
            dropped_tx.send(()).expect("test receiver remains");
        });
        let dropped = dropped_rx.recv_timeout(Duration::from_millis(100));
        release.store(true, Ordering::SeqCst);
        assert!(dropped.is_ok(), "dropping walks waited for its worker");
    }
}
