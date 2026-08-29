// @okf-doc: /decisions/0040-agent-subscriptions-and-hooks.md
//! The agent register of one workspace (ADR 0040): which harness sessions
//! subscribed, what each has been shown, and what each is waiting for.
//!
//! The register is an append-only JSONL file beside the thread store,
//! written one whole line per event so a viewer, a headless `--mcp`, and a
//! hook process may all append. It holds no thread content. Deleting it
//! forgets every subscription and nothing else.
//!
//! A thread is *pending* for a subscriber when it is open and its newest
//! message is someone else's ([`Thread::pending_for`]); it is
//! *deliverable* when that message has not been shown to the subscriber
//! yet. [`Blob::render`] renders what a hook hands the model.

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::annotations::{Status, Thread, ThreadId};
use crate::bond::{self, Bond, Process};

/// File name of the register inside the workspace state directory.
pub const AGENTS_FILE: &str = "agents.jsonl";

const FORMAT_VERSION: u32 = 1;

/// A harness session that subscribed to the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscriber {
    id: String,
    kind: String,
    name: Option<String>,
    client: Option<String>,
    paths: Vec<PathBuf>,
    created: u64,
    seen: u64,
    checks: u32,
}

impl Subscriber {
    /// The harness session id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The agent type it declared.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The persona it signs as, when it gave one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The MCP client it spoke through, when known.
    #[must_use]
    pub fn client(&self) -> Option<&str> {
        self.client.as_deref()
    }

    /// The files it follows; empty means the whole workspace.
    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// When it subscribed, in Unix seconds.
    #[must_use]
    pub fn created(&self) -> u64 {
        self.created
    }

    /// When it was last heard from, in Unix seconds.
    #[must_use]
    pub fn seen(&self) -> u64 {
        self.seen
    }

    /// `name (type)`, or `type` alone when it has no name.
    #[must_use]
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) => format!("{name} ({})", self.kind),
            None => self.kind.clone(),
        }
    }

    /// Whether `thread` is in this subscriber's scope: on a followed
    /// path, or one it has posted in.
    #[must_use]
    pub fn covers(&self, thread: &Thread) -> bool {
        self.paths.is_empty()
            || self.paths.iter().any(|p| p == thread.path())
            || thread.has_reply_from(&self.id)
    }
}

/// What a watch waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchWhen {
    /// A message from someone other than the watcher.
    Message,
    /// The thread being resolved.
    Resolved,
}

impl fmt::Display for WatchWhen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Message => "message",
            Self::Resolved => "resolved",
        })
    }
}

/// A one-shot wake-up: when thread `on` reaches `when`, remind
/// `subscriber` of the `remind` threads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    subscriber: String,
    on: ThreadId,
    when: WatchWhen,
    remind: Vec<ThreadId>,
    created: u64,
}

impl Watch {
    /// The subscriber that asked.
    #[must_use]
    pub fn subscriber(&self) -> &str {
        &self.subscriber
    }

    /// The thread watched.
    #[must_use]
    pub fn on(&self) -> &ThreadId {
        &self.on
    }

    /// What it waits for.
    #[must_use]
    pub fn when(&self) -> WatchWhen {
        self.when
    }

    /// The threads to re-deliver when it fires.
    #[must_use]
    pub fn remind(&self) -> &[ThreadId] {
        &self.remind
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    Subscribe {
        v: u32,
        id: String,
        kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client: Option<String>,
        paths: Vec<PathBuf>,
        created: u64,
    },
    Unsubscribe {
        v: u32,
        id: String,
        created: u64,
    },
    Seen {
        v: u32,
        id: String,
        created: u64,
    },
    /// The newest message of `thread`, written `at`, was shown to `id`.
    Deliver {
        v: u32,
        id: String,
        thread: ThreadId,
        at: u64,
        created: u64,
    },
    /// A hook asked for `id` and found delivered threads still pending.
    Check {
        v: u32,
        id: String,
        created: u64,
    },
    /// A reminder went out; the check count starts over.
    Nag {
        v: u32,
        id: String,
        created: u64,
    },
    Watch {
        v: u32,
        id: String,
        on: ThreadId,
        when: WatchWhen,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remind: Vec<ThreadId>,
        created: u64,
    },
    /// The watch was removed, whether it fired or was cancelled.
    Unwatch {
        v: u32,
        id: String,
        on: ThreadId,
        created: u64,
    },
    /// The `hello` hook for session `id` ran under `processes` (ADR 0041).
    Bond {
        v: u32,
        id: String,
        processes: Vec<Process>,
        created: u64,
    },
}

/// The subscribers of one workspace, backed by an append-only JSONL file.
#[derive(Debug)]
pub struct Register {
    path: PathBuf,
    subscribers: Vec<Subscriber>,
    deliveries: HashMap<(String, ThreadId), u64>,
    watches: Vec<Watch>,
    bonds: Vec<Bond>,
}

impl Register {
    /// Load the register at `path`, or start empty when the file is
    /// missing. Subscribers not heard from for `expire_after` before
    /// `now` are dropped with their deliveries and watches.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError`] when the file cannot be read, a line is
    /// not a known event, or an event names a subscriber the file never
    /// created.
    pub fn open(
        path: impl Into<PathBuf>,
        now: u64,
        expire_after: Duration,
    ) -> Result<Self, RegisterError> {
        let path = path.into();
        let mut register = Self {
            path,
            subscribers: Vec::new(),
            deliveries: HashMap::new(),
            watches: Vec::new(),
            bonds: Vec::new(),
        };
        let text = match fs::read_to_string(&register.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(register),
            Err(error) => return Err(RegisterError::io(&register.path, error)),
        };
        for (index, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event: Event = serde_json::from_str(line)
                .map_err(|error| RegisterError::parse(index + 1, error.to_string()))?;
            register
                .apply(event)
                .map_err(|error| RegisterError::parse(index + 1, error.to_string()))?;
        }
        let cutoff = now.saturating_sub(expire_after.as_secs());
        let expired: Vec<String> = register
            .subscribers
            .iter()
            .filter(|s| s.seen < cutoff)
            .map(|s| s.id.clone())
            .collect();
        for id in expired {
            tracing::info!(id, "subscription expired");
            register.forget(&id);
        }
        register.bonds.retain(|b| b.created() >= cutoff);
        Ok(register)
    }

    /// Where the JSONL file lives.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every live subscriber, oldest first.
    #[must_use]
    pub fn subscribers(&self) -> &[Subscriber] {
        &self.subscribers
    }

    /// The subscriber with `id`, if any.
    #[must_use]
    pub fn subscriber(&self, id: &str) -> Option<&Subscriber> {
        self.subscribers.iter().find(|s| s.id == id)
    }

    /// Every watch, oldest first.
    #[must_use]
    pub fn watches(&self) -> &[Watch] {
        &self.watches
    }

    /// Every session bond still live, oldest first.
    #[must_use]
    pub fn bonds(&self) -> &[Bond] {
        &self.bonds
    }

    /// The session id bonded to the nearest of `ancestors`, when exactly
    /// one session recorded it ([`bond::session_for`]).
    #[must_use]
    pub fn session_for(&self, ancestors: &[Process]) -> Option<&str> {
        bond::session_for(&self.bonds, ancestors)
    }

    /// Record that the `hello` hook for session `id` ran under
    /// `processes`. The session need not be subscribed.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError`] when the file cannot be appended to.
    pub fn bond(
        &mut self,
        id: &str,
        processes: Vec<Process>,
        now: u64,
    ) -> Result<(), RegisterError> {
        self.commit(Event::Bond {
            v: FORMAT_VERSION,
            id: id.to_owned(),
            processes,
            created: now,
        })
    }

    /// The watches on `thread`.
    pub fn watches_on<'a>(&'a self, thread: &'a ThreadId) -> impl Iterator<Item = &'a Watch> + 'a {
        self.watches.iter().filter(move |w| w.on == *thread)
    }

    /// Subscribe `id` as `kind` following `paths`, or refresh the paths of
    /// an existing subscription.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError`] when `id` is already subscribed as
    /// another type, or the file cannot be appended to.
    pub fn subscribe(
        &mut self,
        id: &str,
        kind: &str,
        name: Option<&str>,
        client: Option<&str>,
        paths: Vec<PathBuf>,
        now: u64,
    ) -> Result<(), RegisterError> {
        if let Some(existing) = self.subscriber(id)
            && existing.kind != kind
        {
            return Err(RegisterError {
                kind: ErrorKind::KindFixed(id.to_owned(), existing.kind.clone()),
            });
        }
        self.commit(Event::Subscribe {
            v: FORMAT_VERSION,
            id: id.to_owned(),
            kind: kind.to_owned(),
            name: name.map(str::to_owned),
            client: client.map(str::to_owned),
            paths,
            created: now,
        })
    }

    /// Remove `id`, its deliveries, and its watches.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError`] when `id` is unknown or the file cannot be
    /// appended to.
    pub fn unsubscribe(&mut self, id: &str, now: u64) -> Result<(), RegisterError> {
        self.known(id)?;
        self.commit(Event::Unsubscribe {
            v: FORMAT_VERSION,
            id: id.to_owned(),
            created: now,
        })
    }

    /// Note that `id` was heard from at `now`.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError`] when `id` is unknown or the file cannot be
    /// appended to.
    pub fn touch(&mut self, id: &str, now: u64) -> Result<(), RegisterError> {
        self.known(id)?;
        self.commit(Event::Seen {
            v: FORMAT_VERSION,
            id: id.to_owned(),
            created: now,
        })
    }

    /// The threads among `threads` that are pending for `subscriber` and
    /// whose newest message it has not been shown, oldest first.
    pub fn deliverable<'a>(
        &self,
        subscriber: &Subscriber,
        threads: impl IntoIterator<Item = &'a Thread>,
    ) -> Vec<&'a Thread> {
        threads
            .into_iter()
            .filter(|t| subscriber.covers(t) && t.pending_for(&subscriber.id))
            .filter(|t| {
                let key = (subscriber.id.clone(), t.id().clone());
                self.deliveries.get(&key) != Some(&t.newest().1)
            })
            .collect()
    }

    /// The threads among `threads` that are pending for `subscriber` and
    /// were already shown to it: the ones a reminder would name.
    pub fn stale<'a>(
        &self,
        subscriber: &Subscriber,
        threads: impl IntoIterator<Item = &'a Thread>,
    ) -> Vec<&'a Thread> {
        threads
            .into_iter()
            .filter(|t| subscriber.covers(t) && t.pending_for(&subscriber.id))
            .filter(|t| {
                let key = (subscriber.id.clone(), t.id().clone());
                self.deliveries.get(&key) == Some(&t.newest().1)
            })
            .collect()
    }

    /// Record that `thread`'s newest message was shown to `id`.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError`] when `id` is unknown or the file cannot be
    /// appended to.
    pub fn deliver(&mut self, id: &str, thread: &Thread, now: u64) -> Result<(), RegisterError> {
        self.known(id)?;
        self.commit(Event::Deliver {
            v: FORMAT_VERSION,
            id: id.to_owned(),
            thread: thread.id().clone(),
            at: thread.newest().1,
            created: now,
        })
    }

    /// Count a hook check that found delivered threads still pending.
    /// Returns whether a reminder is now due under `nag_after` (never
    /// when it is zero); a due reminder is recorded as sent.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError`] when `id` is unknown or the file cannot be
    /// appended to.
    pub fn check(&mut self, id: &str, nag_after: u32, now: u64) -> Result<bool, RegisterError> {
        self.known(id)?;
        self.commit(Event::Check {
            v: FORMAT_VERSION,
            id: id.to_owned(),
            created: now,
        })?;
        let checks = self.subscriber(id).map_or(0, |s| s.checks);
        if nag_after == 0 || checks < nag_after {
            return Ok(false);
        }
        self.commit(Event::Nag {
            v: FORMAT_VERSION,
            id: id.to_owned(),
            created: now,
        })?;
        Ok(true)
    }

    /// Have `id` woken when `on` reaches `when`, reminded of `remind`.
    /// A second watch by the same subscriber on the same thread replaces
    /// the first.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError`] when `id` is unknown or the file cannot be
    /// appended to.
    pub fn watch(
        &mut self,
        id: &str,
        on: &ThreadId,
        when: WatchWhen,
        remind: Vec<ThreadId>,
        now: u64,
    ) -> Result<(), RegisterError> {
        self.known(id)?;
        self.commit(Event::Watch {
            v: FORMAT_VERSION,
            id: id.to_owned(),
            on: on.clone(),
            when,
            remind,
            created: now,
        })
    }

    /// Remove `id`'s watch on `on`.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError`] when `id` is unknown, has no such watch,
    /// or the file cannot be appended to.
    pub fn unwatch(&mut self, id: &str, on: &ThreadId, now: u64) -> Result<(), RegisterError> {
        self.known(id)?;
        if !self
            .watches
            .iter()
            .any(|w| w.subscriber == id && w.on == *on)
        {
            return Err(RegisterError {
                kind: ErrorKind::UnknownWatch(id.to_owned(), on.clone()),
            });
        }
        self.commit(Event::Unwatch {
            v: FORMAT_VERSION,
            id: id.to_owned(),
            on: on.clone(),
            created: now,
        })
    }

    /// The watches of `id` whose condition `threads` now satisfy, removed
    /// from the register as they are returned. A watch on a thread no
    /// longer in `threads` is dropped silently.
    ///
    /// # Errors
    ///
    /// Returns [`RegisterError`] when the file cannot be appended to.
    pub fn fire<'a>(
        &mut self,
        id: &str,
        threads: &'a [Thread],
        now: u64,
    ) -> Result<Vec<Fired<'a>>, RegisterError> {
        let mine: Vec<Watch> = self
            .watches
            .iter()
            .filter(|w| w.subscriber == id)
            .cloned()
            .collect();
        let mut fired = Vec::new();
        for watch in mine {
            let Some(thread) = threads.iter().find(|t| t.id() == &watch.on) else {
                self.unwatch(id, &watch.on, now)?;
                continue;
            };
            let (author, at) = thread.newest();
            let due = match watch.when {
                WatchWhen::Resolved => thread.status() != Status::Open,
                WatchWhen::Message => at > watch.created && author.id() != Some(id),
            };
            if due {
                self.unwatch(id, &watch.on, now)?;
                fired.push(Fired { watch, thread });
            }
        }
        Ok(fired)
    }

    fn known(&self, id: &str) -> Result<(), RegisterError> {
        self.subscriber(id)
            .map(|_| ())
            .ok_or_else(|| RegisterError {
                kind: ErrorKind::UnknownSubscriber(id.to_owned()),
            })
    }

    fn forget(&mut self, id: &str) {
        self.subscribers.retain(|s| s.id != id);
        self.deliveries.retain(|(s, _), _| s != id);
        self.watches.retain(|w| w.subscriber != id);
    }

    fn commit(&mut self, event: Event) -> Result<(), RegisterError> {
        let mut line = serde_json::to_string(&event)
            .map_err(|error| RegisterError::parse(0, error.to_string()))?;
        self.apply(event)?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| RegisterError::io(parent, error))?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| RegisterError::io(&self.path, error))?;
        // One write per event, as the thread store does (ADR 0032).
        line.push('\n');
        file.write_all(line.as_bytes())
            .map_err(|error| RegisterError::io(&self.path, error))
    }

    fn apply(&mut self, event: Event) -> Result<(), RegisterError> {
        match event {
            Event::Subscribe {
                id,
                kind,
                name,
                client,
                paths,
                created,
                ..
            } => match self.subscribers.iter_mut().find(|s| s.id == id) {
                Some(existing) => {
                    if existing.kind != kind {
                        return Err(RegisterError {
                            kind: ErrorKind::KindFixed(id, existing.kind.clone()),
                        });
                    }
                    existing.paths = paths;
                    existing.seen = existing.seen.max(created);
                    if name.is_some() {
                        existing.name = name;
                    }
                    if client.is_some() {
                        existing.client = client;
                    }
                }
                None => self.subscribers.push(Subscriber {
                    id,
                    kind,
                    name,
                    client,
                    paths,
                    created,
                    seen: created,
                    checks: 0,
                }),
            },
            Event::Unsubscribe { id, .. } => {
                self.known(&id)?;
                self.forget(&id);
            }
            Event::Seen { id, created, .. } => {
                let s = self.subscriber_mut(&id)?;
                s.seen = s.seen.max(created);
            }
            Event::Deliver {
                id,
                thread,
                at,
                created,
                ..
            } => {
                let s = self.subscriber_mut(&id)?;
                s.seen = s.seen.max(created);
                s.checks = 0;
                self.deliveries.insert((id, thread), at);
            }
            Event::Check { id, created, .. } => {
                let s = self.subscriber_mut(&id)?;
                s.seen = s.seen.max(created);
                s.checks = s.checks.saturating_add(1);
            }
            Event::Nag { id, .. } => {
                self.subscriber_mut(&id)?.checks = 0;
            }
            Event::Watch {
                id,
                on,
                when,
                remind,
                created,
                ..
            } => {
                self.known(&id)?;
                self.watches.retain(|w| !(w.subscriber == id && w.on == on));
                self.watches.push(Watch {
                    subscriber: id,
                    on,
                    when,
                    remind,
                    created,
                });
            }
            Event::Unwatch { id, on, .. } => {
                self.known(&id)?;
                self.watches.retain(|w| !(w.subscriber == id && w.on == on));
            }
            Event::Bond {
                id,
                processes,
                created,
                ..
            } => self.bonds.push(Bond::new(id, processes, created)),
        }
        Ok(())
    }

    fn subscriber_mut(&mut self, id: &str) -> Result<&mut Subscriber, RegisterError> {
        self.subscribers
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| RegisterError {
                kind: ErrorKind::UnknownSubscriber(id.to_owned()),
            })
    }
}

/// What a hook hands the model: fired watches, newly pending threads,
/// and a reminder, rendered as plain text sized by `max_lines`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Blob<'a> {
    /// Watches that fired, each with the threads it asked to be reminded of.
    pub fired: Vec<(Fired<'a>, Vec<&'a Thread>)>,
    /// Threads delivered for the first time at this message.
    pub fresh: Vec<&'a Thread>,
    /// Threads already shown and still unanswered, named in a reminder.
    pub reminder: Vec<&'a Thread>,
}

impl Blob<'_> {
    /// Whether there is nothing to say.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fired.is_empty() && self.fresh.is_empty() && self.reminder.is_empty()
    }

    /// Render for `subscriber`, keeping the full form within `max_lines`
    /// lines and listing the rest by id and place.
    #[must_use]
    pub fn render(&self, subscriber: &Subscriber, max_lines: usize) -> String {
        let mut out = Vec::new();
        let count = self.fired.iter().map(|(_, r)| r.len() + 1).sum::<usize>() + self.fresh.len();
        if count > 0 {
            out.push(format!(
                "FATHOMABLE: {count} review thread{} need{} your reply (you are {}).",
                if count == 1 { "" } else { "s" },
                if count == 1 { "s" } else { "" },
                subscriber.label()
            ));
            out.push(
                "Act on each, then answer every thread in ONE `thread_reply` call with \
                 `replies` (pass line/end_line if you moved the lines). Full history: \
                 `annotations_list`; more pending: `threads_pending`."
                    .to_owned(),
            );
        }
        let mut full: Vec<(&Thread, Option<String>)> = Vec::new();
        for (fired, remind) in &self.fired {
            let head = format!(
                "watch fired: {} {} ({})",
                fired.thread.id(),
                fired.watch.when(),
                fired.thread.newest().0
            );
            full.push((fired.thread, Some(head)));
            for thread in remind {
                full.push((thread, None));
            }
        }
        for thread in &self.fresh {
            full.push((thread, None));
        }
        let mut listed: Vec<&Thread> = Vec::new();
        for (thread, head) in full {
            let block = describe(thread, head.as_deref());
            if out.len() + block.len() + 1 > max_lines && !listed.is_empty()
                || out.len() + block.len() + 1 > max_lines
            {
                listed.push(thread);
                continue;
            }
            out.push(String::new());
            out.extend(block);
        }
        if !listed.is_empty() {
            out.push(String::new());
            out.push(format!("{} more; call `threads_pending`:", listed.len()));
            for thread in listed {
                out.push(format!(
                    "  {} {}:{}",
                    thread.id(),
                    thread.path().display(),
                    thread.range()
                ));
            }
        }
        if !self.reminder.is_empty() {
            if !out.is_empty() {
                out.push(String::new());
            }
            let places: Vec<String> = self
                .reminder
                .iter()
                .map(|t| format!("{}:{}", t.path().display(), t.range()))
                .collect();
            out.push(format!(
                "FATHOMABLE reminder: {} thread{} still unanswered: {}",
                self.reminder.len(),
                if self.reminder.len() == 1 { "" } else { "s" },
                places.join(", ")
            ));
        }
        out.join("\n")
    }
}

/// One thread as the model sees it: a header, up to six snippet lines,
/// and its newest two messages.
fn describe(thread: &Thread, head: Option<&str>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(head) = head {
        lines.push(format!("── {head}"));
    }
    let messages = 1 + thread.replies().len();
    lines.push(format!(
        "── thread {} · {}:{} · {} · {messages} msg{}",
        thread.id(),
        thread.path().display(),
        thread.range(),
        match thread.status() {
            Status::Open => "open",
            Status::Resolved => "resolved",
            Status::AutoResolved => "auto-resolved",
        },
        if messages == 1 { "" } else { "s" }
    ));
    let start = thread.range().start();
    let snippet: Vec<&str> = thread.snippet().lines().collect();
    for (i, text) in snippet.iter().take(6).enumerate() {
        lines.push(format!("   {:>4} │ {text}", start + i));
    }
    if snippet.len() > 6 {
        lines.push(format!("        │ … {} more line(s)", snippet.len() - 6));
    }
    let mut recent: Vec<(String, &str)> = thread
        .replies()
        .iter()
        .rev()
        .take(2)
        .map(|r| (r.author().to_string(), r.body()))
        .collect();
    recent.reverse();
    if recent.len() < 2 {
        recent.insert(0, ("user".to_owned(), thread.comment()));
    }
    for (who, body) in recent {
        let mut body = body.lines();
        let first = body.next().unwrap_or_default();
        lines.push(format!("   {who}: {first}"));
        for more in body.take(3) {
            lines.push(format!("     {more}"));
        }
    }
    lines
}

/// A watch that fired, with the thread that satisfied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fired<'a> {
    /// The watch.
    pub watch: Watch,
    /// The thread it was on, as it is now.
    pub thread: &'a Thread,
}

#[derive(Debug)]
enum ErrorKind {
    Io(PathBuf, io::Error),
    Parse(usize, String),
    UnknownSubscriber(String),
    UnknownWatch(String, ThreadId),
    KindFixed(String, String),
}

/// Why the register could not be read or written.
#[derive(Debug)]
pub struct RegisterError {
    kind: ErrorKind,
}

impl RegisterError {
    fn io(path: &Path, error: io::Error) -> Self {
        Self {
            kind: ErrorKind::Io(path.to_path_buf(), error),
        }
    }

    fn parse(line: usize, message: String) -> Self {
        Self {
            kind: ErrorKind::Parse(line, message),
        }
    }

    /// Whether the cause was an I/O failure.
    #[must_use]
    pub fn is_io(&self) -> bool {
        matches!(self.kind, ErrorKind::Io(..))
    }
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ErrorKind::Io(path, error) => write!(f, "{}: {error}", path.display()),
            ErrorKind::Parse(line, message) => write!(f, "{AGENTS_FILE} line {line}: {message}"),
            ErrorKind::UnknownSubscriber(id) => {
                write!(
                    f,
                    "session {id} is not subscribed; call follow with id and type"
                )
            }
            ErrorKind::UnknownWatch(id, on) => write!(f, "session {id} has no watch on {on}"),
            ErrorKind::KindFixed(id, kind) => {
                write!(
                    f,
                    "session {id} is subscribed as {kind}; a type cannot change"
                )
            }
        }
    }
}

impl std::error::Error for RegisterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.kind {
            ErrorKind::Io(_, error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use crate::annotations::{Author, Draft, LineRange, Reply, Store};

    use super::{Blob, Register, WatchWhen};
    use crate::bond::Process;

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn a_bond_names_the_session_and_expires_with_the_register() -> TestResult {
        let dir = TempDir::new("bond")?;
        let path = dir.0.join("agents.jsonl");
        let day = Duration::from_hours(24);
        let mut register = Register::open(&path, 1_000, day)?;
        register.bond("s-1", vec![Process::new(20, 5), Process::new(10, 1)], 1_000)?;
        let mine = [Process::new(30, 9), Process::new(20, 5)];
        assert_eq!(register.session_for(&mine), Some("s-1"));

        let reopened = Register::open(&path, 2_000, day)?;
        assert_eq!(reopened.bonds().len(), 1);
        assert_eq!(reopened.session_for(&mine), Some("s-1"));
        let later = Register::open(&path, 1_000 + day.as_secs() + 1, day)?;
        assert_eq!(later.session_for(&mine), None);
        Ok(())
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> std::io::Result<Self> {
            let dir = std::env::temp_dir()
                .join(format!("fathomable-agents-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir)?;
            Ok(Self(dir))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const TEXT: &str = "one\ntwo\nthree\nfour\n";
    const DAY: Duration = Duration::from_hours(24);

    fn store_with_thread(
        dir: &TempDir,
    ) -> Result<(Store, crate::annotations::ThreadId), Box<dyn Error>> {
        let mut store = Store::open(dir.0.join("threads.jsonl"))?;
        let id = store.annotate(
            Draft::new(Path::new("a.md"), LineRange::new(2, 3), "tighten this"),
            TEXT,
            100,
        )?;
        Ok((store, id))
    }

    #[test]
    fn a_message_is_delivered_once_until_someone_else_speaks() -> TestResult {
        let dir = TempDir::new("once")?;
        let (mut store, id) = store_with_thread(&dir)?;
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", Some("bot"), None, vec![], 200)?;
        let sub = reg.subscriber("s-1").ok_or("no subscriber")?.clone();
        let due = reg.deliverable(&sub, store.threads());
        assert_eq!(due.len(), 1);
        reg.deliver("s-1", due[0], 201)?;
        assert!(reg.deliverable(&sub, store.threads()).is_empty());
        assert_eq!(reg.stale(&sub, store.threads()).len(), 1);
        let me = Author::agent("bot").subscribed("s-1", "coder");
        store.reply(&id, Reply::new(me, 202, "done"))?;
        assert!(reg.deliverable(&sub, store.threads()).is_empty());
        assert!(reg.stale(&sub, store.threads()).is_empty());
        let other = Author::agent("rev").subscribed("s-2", "reviewer");
        store.reply(&id, Reply::new(other, 203, "not quite"))?;
        assert_eq!(reg.deliverable(&sub, store.threads()).len(), 1);
        // Reloading keeps the delivery and the subscription.
        let again = Register::open(dir.0.join("agents.jsonl"), 300, DAY)?;
        assert_eq!(again.subscribers().len(), 1);
        assert_eq!(again.deliverable(&sub, store.threads()).len(), 1);
        Ok(())
    }

    #[test]
    fn scope_is_followed_paths_or_own_threads() -> TestResult {
        let dir = TempDir::new("scope")?;
        let (mut store, id) = store_with_thread(&dir)?;
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", None, None, vec![PathBuf::from("b.md")], 200)?;
        let sub = reg.subscriber("s-1").ok_or("no subscriber")?.clone();
        assert!(reg.deliverable(&sub, store.threads()).is_empty());
        let me = Author::agent("bot").subscribed("s-1", "coder");
        store.reply(&id, Reply::new(me, 201, "I was here"))?;
        store.reply(&id, Reply::new(Author::User, 202, "and?"))?;
        assert_eq!(reg.deliverable(&sub, store.threads()).len(), 1);
        reg.subscribe("s-1", "coder", None, None, vec![], 203)?;
        let sub = reg.subscriber("s-1").ok_or("no subscriber")?.clone();
        assert!(sub.paths().is_empty());
        assert_eq!(
            reg.subscribe("s-1", "reviewer", None, None, vec![], 204)
                .err()
                .map(|e| e.to_string()),
            Some("session s-1 is subscribed as coder; a type cannot change".to_owned())
        );
        Ok(())
    }

    #[test]
    fn nags_count_checks_and_expiry_drops_silent_sessions() -> TestResult {
        let dir = TempDir::new("nag")?;
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", None, None, vec![], 200)?;
        assert!(!reg.check("s-1", 3, 201)?);
        assert!(!reg.check("s-1", 3, 202)?);
        assert!(reg.check("s-1", 3, 203)?);
        assert!(!reg.check("s-1", 3, 204)?);
        assert!(!reg.check("s-1", 0, 205)?);
        reg.touch("s-1", 300)?;
        let later = Register::open(dir.0.join("agents.jsonl"), 300 + DAY.as_secs() + 1, DAY)?;
        assert!(later.subscribers().is_empty());
        let sooner = Register::open(dir.0.join("agents.jsonl"), 300 + DAY.as_secs() - 1, DAY)?;
        assert_eq!(sooner.subscribers().len(), 1);
        reg.unsubscribe("s-1", 301)?;
        assert_eq!(reg.touch("s-1", 302).ok(), None);
        Ok(())
    }

    #[test]
    fn a_watch_fires_once_and_carries_its_reminders() -> TestResult {
        let dir = TempDir::new("watch")?;
        let (mut store, id) = store_with_thread(&dir)?;
        let other = store.annotate(
            Draft::new(Path::new("a.md"), LineRange::new(1, 1), "also"),
            TEXT,
            101,
        )?;
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", None, None, vec![], 200)?;
        reg.watch("s-1", &id, WatchWhen::Resolved, vec![other.clone()], 201)?;
        reg.watch("s-1", &other, WatchWhen::Message, vec![], 201)?;
        assert!(reg.fire("s-1", store.threads(), 202)?.is_empty());
        let me = Author::agent("bot").subscribed("s-1", "coder");
        store.reply(&other, Reply::new(me, 203, "mine"))?;
        assert!(
            reg.fire("s-1", store.threads(), 204)?.is_empty(),
            "own message"
        );
        store.reply(&other, Reply::new(Author::User, 205, "theirs"))?;
        let fired = reg.fire("s-1", store.threads(), 206)?;
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].watch.on(), &other);
        assert_eq!(reg.watches().len(), 1);
        store.resolve(&id, Author::User, 207)?;
        let fired = reg.fire("s-1", store.threads(), 208)?;
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].watch.remind(), std::slice::from_ref(&other));
        assert!(reg.watches().is_empty());
        assert_eq!(reg.unwatch("s-1", &id, 209).ok(), None);
        Ok(())
    }

    #[test]
    fn the_blob_is_bounded_and_says_what_to_call() -> TestResult {
        let dir = TempDir::new("blob")?;
        let (mut store, id) = store_with_thread(&dir)?;
        store.reply(
            &id,
            Reply::new(
                Author::agent("x").subscribed("s-9", "reviewer"),
                150,
                "hm\nsecond line",
            ),
        )?;
        for n in 0..5 {
            store.annotate(
                Draft::new(Path::new("b.md"), LineRange::new(1, 4), format!("note {n}")),
                TEXT,
                160 + n,
            )?;
        }
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", Some("bot"), None, vec![], 200)?;
        let sub = reg.subscriber("s-1").ok_or("no subscriber")?.clone();
        let fresh = reg.deliverable(&sub, store.threads());
        let blob = Blob {
            fresh,
            ..Blob::default()
        };
        let text = blob.render(&sub, 20);
        assert!(
            text.starts_with("FATHOMABLE: 6 review threads need your reply (you are bot (coder)).")
        );
        assert!(text.contains("thread_reply"));
        assert!(text.contains("x (reviewer): hm"));
        assert!(text.contains("     second line"));
        assert!(text.contains("more; call `threads_pending`"));
        assert!(text.lines().count() <= 20 + 6, "{text}");
        let empty = Blob::default();
        assert!(empty.is_empty());
        assert_eq!(empty.render(&sub, 40), "");
        let reminder = Blob {
            reminder: store.threads().iter().take(2).collect(),
            ..Blob::default()
        };
        let text = reminder.render(&sub, 40);
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("2 threads still unanswered: a.md:2-3, b.md:1-4"));
        Ok(())
    }
}
