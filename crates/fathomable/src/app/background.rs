// @okf-doc: /decisions/0012-workspace-mode.md
//! One cancellable worker with a replaceable pending request and bounded results.

#![expect(
    clippy::expect_used,
    reason = "poisoned mailboxes indicate a programming failure; tests require their synchronization"
)]

use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use fathomable_core::workspace::Cancellation;
use tokio::sync::mpsc;

#[derive(Debug)]
struct Request<I> {
    generation: u64,
    cancellation: Cancellation,
    input: I,
}

#[derive(Debug)]
struct Mailbox<I> {
    request: Option<Request<I>>,
    stopped: bool,
}

/// At most one executing job, one latest request, and one undelivered result.
#[derive(Debug)]
pub(super) struct Worker<I, O> {
    mailbox: Arc<(Mutex<Mailbox<I>>, Condvar)>,
    receiver: Option<mpsc::Receiver<(u64, O)>>,
    cancellation: Cancellation,
    generation: u64,
    pending: bool,
    execute: fn(I, Cancellation) -> O,
}

impl<I: Send + 'static, O: Send + 'static> Worker<I, O> {
    pub(super) fn new(execute: fn(I, Cancellation) -> O) -> Self {
        Self {
            mailbox: Arc::new((
                Mutex::new(Mailbox {
                    request: None,
                    stopped: false,
                }),
                Condvar::new(),
            )),
            receiver: None,
            cancellation: Cancellation::default(),
            generation: 0,
            pending: false,
            execute,
        }
    }

    pub(super) fn submit(&mut self, input: I) -> io::Result<()> {
        if self.receiver.is_none() {
            let (sender, receiver) = mpsc::channel(1);
            let mailbox = Arc::clone(&self.mailbox);
            let execute = self.execute;
            thread::Builder::new()
                .name("workspace scan".to_owned())
                .spawn(move || {
                    loop {
                        let request = {
                            let (lock, wake) = &*mailbox;
                            let mut state = lock.lock().expect("workspace worker mailbox poisoned");
                            while state.request.is_none() && !state.stopped {
                                state =
                                    wake.wait(state).expect("workspace worker mailbox poisoned");
                            }
                            if state.stopped {
                                return;
                            }
                            state.request.take().expect("notified worker has a request")
                        };
                        let output = execute(request.input, request.cancellation.clone());
                        if request.cancellation.is_cancelled() {
                            continue;
                        }
                        if sender.blocking_send((request.generation, output)).is_err() {
                            return;
                        }
                    }
                })?;
            self.receiver = Some(receiver);
        }
        self.cancel();
        self.cancellation = Cancellation::default();
        self.pending = true;
        let (lock, wake) = &*self.mailbox;
        lock.lock()
            .expect("workspace worker mailbox poisoned")
            .request = Some(Request {
            generation: self.generation,
            cancellation: self.cancellation.clone(),
            input,
        });
        wake.notify_one();
        Ok(())
    }

    pub(super) fn cancel(&mut self) {
        self.cancellation.cancel();
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
        self.mailbox
            .0
            .lock()
            .expect("workspace worker mailbox poisoned")
            .request = None;
    }

    pub(super) const fn pending(&self) -> bool {
        self.pending
    }

    pub(super) fn poll(&mut self) -> io::Result<Option<O>> {
        let Some(receiver) = &mut self.receiver else {
            return Ok(None);
        };
        loop {
            match receiver.try_recv() {
                Ok((generation, output)) if generation == self.generation => {
                    self.pending = false;
                    return Ok(Some(output));
                }
                Ok(_) => {}
                Err(mpsc::error::TryRecvError::Empty) => return Ok(None),
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.pending = false;
                    self.receiver = None;
                    return Err(io::Error::other("workspace scan worker stopped"));
                }
            }
        }
    }
}

impl<I, O> Drop for Worker<I, O> {
    fn drop(&mut self) {
        self.cancellation.cancel();
        let (lock, wake) = &*self.mailbox;
        let mut state = lock.lock().expect("workspace worker mailbox poisoned");
        state.stopped = true;
        state.request = None;
        wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::Worker;
    use fathomable_core::workspace::Cancellation;

    struct Job {
        id: usize,
        started: mpsc::Sender<usize>,
        release: mpsc::Receiver<()>,
    }

    fn execute(job: Job, _: Cancellation) -> usize {
        let Job {
            id,
            started,
            release,
        } = job;
        started.send(id).expect("test observes starts");
        release.recv().expect("test releases worker");
        id
    }

    #[test]
    fn supersession_keeps_only_the_latest_pending_request() {
        let (started, starts) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let mut worker = Worker::new(execute);
        worker
            .submit(Job {
                id: 1,
                started: started.clone(),
                release: wait,
            })
            .expect("submit");
        assert_eq!(
            starts
                .recv_timeout(Duration::from_secs(2))
                .expect("started"),
            1
        );
        let (_, obsolete) = mpsc::channel();
        worker
            .submit(Job {
                id: 2,
                started: started.clone(),
                release: obsolete,
            })
            .expect("replace");
        let (last_release, last_wait) = mpsc::channel();
        worker
            .submit(Job {
                id: 3,
                started,
                release: last_wait,
            })
            .expect("replace again");
        release.send(()).expect("release old generation");
        assert_eq!(
            starts
                .recv_timeout(Duration::from_secs(2))
                .expect("latest starts"),
            3
        );
        last_release.send(()).expect("release latest");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = worker.poll().expect("poll") {
                assert_eq!(result, 3);
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!worker.pending());
    }

    #[test]
    fn drop_does_not_join_a_blocked_operation() {
        let (started, starts) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let mut worker = Worker::new(execute);
        worker
            .submit(Job {
                id: 1,
                started,
                release: wait,
            })
            .expect("submit");
        starts
            .recv_timeout(Duration::from_secs(2))
            .expect("started");
        let before = Instant::now();
        drop(worker);
        assert!(before.elapsed() < Duration::from_millis(100));
        release.send(()).expect("release detached operation");
    }
}
