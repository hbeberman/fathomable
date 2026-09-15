// @okf-doc: /decisions/0040-agent-subscriptions-and-hooks.md
//! The agent register of one workspace (ADR 0040): which harness sessions
//! subscribed, what each has been shown, and what each is waiting for.
//!
//! The register is an append-only JSONL file beside the thread store,
//! written one whole line per event so a viewer, a headless `--mcp`, and a
//! hook process may all append. It holds no thread content. Deleting it
//! forgets every subscription and nothing else.
//!
//! A thread is *pending* when it is open and the user has the last word
//! ([`Thread::awaits_agent`], ADR 0058); it is *deliverable* to a
//! subscriber when that act has not been shown to it yet.
//! [`Blob::render`] renders what a hook hands the model.

use std::collections::HashMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::annotations::{Author, Status, Thread, ThreadId};
use crate::vocabulary as vocab;

/// File name of the register inside the workspace state directory.
pub(crate) const AGENTS_FILE: &str = "agents.jsonl";

/// The `v` field written to each register line; [`Register::open`]
/// refuses a file of another version (ADR 0062). Bump it when a line's
/// shape changes.
const FORMAT_VERSION: u32 = 1;

/// The `v` of one register line, read before the event itself.
#[derive(Deserialize)]
struct Stamp {
    v: u32,
}

/// A harness session that subscribed to the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscriber {
    id: String,
    kind: String,
    name: Option<String>,
    client: Option<String>,
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
    /// The last act on `thread`, made `at`, was shown to `id`.
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
}

/// The subscribers of one workspace, backed by an append-only JSONL file.
#[derive(Debug)]
pub struct Register {
    path: PathBuf,
    subscribers: Vec<Subscriber>,
    deliveries: HashMap<(String, ThreadId), u64>,
    watches: Vec<Watch>,
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
            let Stamp { v } = serde_json::from_str(line)
                .map_err(|error| RegisterError::parse(index + 1, error.to_string()))?;
            if v != FORMAT_VERSION {
                return Err(RegisterError::version(&register.path, index + 1, v));
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
            register.forget(&id);
        }
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

    /// Subscribe `id` as `kind` to the whole workspace (ADR 0055), or
    /// refresh an existing subscription's name and client.
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

    /// The pending threads among `threads` whose last act `subscriber`
    /// has not been shown, oldest first.
    pub fn deliverable<'a>(
        &self,
        subscriber: &Subscriber,
        threads: impl IntoIterator<Item = &'a Thread>,
    ) -> Vec<&'a Thread> {
        threads
            .into_iter()
            .filter(|t| t.awaits_agent())
            .filter(|t| {
                let key = (subscriber.id.clone(), t.id().clone());
                self.deliveries.get(&key) != Some(&t.last_act().1)
            })
            .collect()
    }

    /// The pending threads among `threads` already shown to `subscriber`:
    /// the ones a reminder would name.
    pub fn stale<'a>(
        &self,
        subscriber: &Subscriber,
        threads: impl IntoIterator<Item = &'a Thread>,
    ) -> Vec<&'a Thread> {
        threads
            .into_iter()
            .filter(|t| t.awaits_agent())
            .filter(|t| {
                let key = (subscriber.id.clone(), t.id().clone());
                self.deliveries.get(&key) == Some(&t.last_act().1)
            })
            .collect()
    }

    /// Record that `thread`'s last act was shown to `id`.
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
            at: thread.last_act().1,
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
                created,
                ..
            } => match self.subscribers.iter_mut().find(|s| s.id == id) {
                Some(existing) => {
                    if existing.kind != kind {
                        return Err(RegisterError {
                            kind: ErrorKind::KindFixed(id, existing.kind.clone()),
                        });
                    }
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
/// and a reminder, rendered as plain text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Blob<'a> {
    /// Watches that fired, each with the threads it asked to be reminded of.
    pub fired: Vec<(Fired<'a>, Vec<&'a Thread>)>,
    /// Threads delivered for the first time at this message.
    pub fresh: Vec<&'a Thread>,
    /// Threads already shown and still unanswered, named in a reminder.
    pub reminder: Vec<&'a Thread>,
    /// Fresh threads past the line budget, named by id alone.
    ///
    /// [`Blob::fit`] fills this; a caller must not record these as
    /// delivered, so that `threads` still returns them.
    pub listed: Vec<&'a Thread>,
}

impl<'a> Blob<'a> {
    /// Whether there is nothing to say.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fired.is_empty() && self.fresh.is_empty() && self.reminder.is_empty()
    }

    /// Every thread the blob shows in full, fired watches first.
    ///
    /// Exactly the threads a caller records as delivered. What
    /// [`Self::fit`] moved to [`Self::listed`] is not among them, so it
    /// stays deliverable through `threads`.
    pub fn shown(&self) -> impl Iterator<Item = &'a Thread> + '_ {
        self.fired
            .iter()
            .flat_map(|(fired, remind)| std::iter::once(fired.thread).chain(remind.iter().copied()))
            .chain(self.fresh.iter().copied())
    }

    /// Move the fresh threads past `max_lines` rendered lines to
    /// [`Self::listed`], where [`Self::render`] names them by id alone.
    ///
    /// Once one thread is over budget every later one is listed too, so
    /// the blob reads in order. Only fresh threads are moved: a fired
    /// watch is consumed when it fires, and the threads it reminds of
    /// need not be pending, so neither could be fetched again — they are
    /// shown in full even when that overruns `max_lines`.
    ///
    /// The blob always shows something, overrunning `max_lines` when it
    /// must: a blob that showed nothing would record no delivery, so the
    /// same id-only list would come back every turn for good.
    ///
    /// Call before [`Self::render`], which renders whatever is left.
    pub fn fit(&mut self, subscriber: &Subscriber, max_lines: usize) {
        let mut used = self.header(subscriber).len();
        // The user's name changes no line count, so any name will do here.
        let name = "user";
        for (fired, remind) in &self.fired {
            used += 1 + describe(fired.thread, Some(&watch_head(fired, name)), name).len();
            for thread in remind {
                used += 1 + describe(thread, None, name).len();
            }
        }
        let mut keep = 0;
        for thread in &self.fresh {
            used += 1 + describe(thread, None, name).len();
            if used > max_lines {
                break;
            }
            keep += 1;
        }
        if self.fired.is_empty() {
            keep = keep.max(1).min(self.fresh.len());
        }
        self.listed = self.fresh.split_off(keep);
    }

    /// Render for `subscriber` everything [`Self::fit`] left in full,
    /// naming the user `user` (ADR 0058).
    #[must_use]
    pub fn render(&self, subscriber: &Subscriber, user: &str) -> String {
        let mut out = self.header(subscriber);
        for (fired, remind) in &self.fired {
            out.push(String::new());
            out.extend(describe(fired.thread, Some(&watch_head(fired, user)), user));
            for thread in remind {
                out.push(String::new());
                out.extend(describe(thread, None, user));
            }
        }
        for thread in &self.fresh {
            out.push(String::new());
            out.extend(describe(thread, None, user));
        }
        if !self.listed.is_empty() {
            out.push(String::new());
            out.push(format!(
                "{} more; call `{}`:",
                self.listed.len(),
                vocab::THREADS.name
            ));
            for thread in &self.listed {
                out.push(format!("  {} {}", thread.id(), thread.place()));
            }
        }
        if !self.reminder.is_empty() {
            if !out.is_empty() {
                out.push(String::new());
            }
            let places: Vec<String> = self.reminder.iter().map(|t| t.place()).collect();
            out.push(format!(
                "FATHOMABLE reminder: {} thread{} still unanswered: {}",
                self.reminder.len(),
                if self.reminder.len() == 1 { "" } else { "s" },
                places.join(", ")
            ));
        }
        out.join("\n")
    }

    /// The two opening lines, counting the listed threads too; empty when
    /// the blob carries no thread to answer.
    fn header(&self, subscriber: &Subscriber) -> Vec<String> {
        let count = self.fired.iter().map(|(_, r)| r.len() + 1).sum::<usize>()
            + self.fresh.len()
            + self.listed.len();
        if count == 0 {
            return Vec::new();
        }
        vec![
            format!(
                "FATHOMABLE: {count} review thread{} need{} your reply (you are {}).",
                if count == 1 { "" } else { "s" },
                if count == 1 { "s" } else { "" },
                subscriber.label()
            ),
            format!(
                "Act on each, then answer every thread in ONE `{reply}` call with \
                 `{replies}` (pass `{line}`/`{end_line}` if you moved the lines). Full \
                 history and anything more pending: `{list}`.",
                reply = vocab::THREAD_REPLY.name,
                replies = vocab::REPLIES,
                line = vocab::LINE,
                end_line = vocab::END_LINE,
                list = vocab::THREADS.name,
            ),
        ]
    }
}

/// The line introducing a thread that a watch fired on.
fn watch_head(fired: &Fired<'_>, user: &str) -> String {
    format!(
        "watch fired: {} {} ({})",
        fired.thread.id(),
        fired.watch.when(),
        who(fired.thread.newest().0, user)
    )
}

/// How an author reads to the model: the user by the configured name,
/// an agent as `name (type)` (ADR 0058).
#[must_use]
pub fn who(author: &Author, user: &str) -> String {
    if author.is_user() {
        user.to_owned()
    } else {
        author.to_string()
    }
}

/// One thread as the model sees it: a header, up to six snippet lines,
/// and its newest two messages — plus the message the user edited, when
/// that edit is the last act and the message is older than those two.
/// An edited message is marked `[edited]` after its author (ADR 0058).
///
/// A message is shown whole. It is the thing the agent must act on,
/// and the prompt is self-contained (ADR 0040): a comment cut short
/// without a word would send the agent off to answer part of it.
/// [`Blob::fit`] keeps the prompt as a whole within `max-lines` by
/// listing later threads by id, not by cutting a message.
fn describe(thread: &Thread, head: Option<&str>, user: &str) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(head) = head {
        lines.push(format!("── {head}"));
    }
    let messages = 1 + thread.replies().len();
    lines.push(format!(
        "── thread {} · {} · {} · {messages} msg{}",
        thread.id(),
        thread.place(),
        match thread.status() {
            Status::Open if thread.proposes_resolution() => "open, proposed",
            Status::Open => "open",
            Status::Resolved => "resolved",
        },
        if messages == 1 { "" } else { "s" }
    ));
    // A thread on the file as a whole has no lines to quote (ADR 0063).
    if let Some(range) = thread.range() {
        let start = range.start();
        let snippet: Vec<&str> = thread.snippet().lines().collect();
        for (i, text) in snippet.iter().take(6).enumerate() {
            lines.push(format!("   {:>4} │ {text}", start + i));
        }
        if snippet.len() > 6 {
            lines.push(format!("        │ … {} more line(s)", snippet.len() - 6));
        }
    }
    // Every message as (who, body, edited), the comment first.
    let all: Vec<(String, &str, Option<u64>)> = std::iter::once((
        who(thread.author(), user),
        thread.comment(),
        thread.comment_edited(),
    ))
    .chain(
        thread
            .replies()
            .iter()
            .map(|r| (who(r.author(), user), r.body(), r.edited())),
    )
    .collect();
    let from = all.len().saturating_sub(2);
    let mut shown: Vec<usize> = (from..all.len()).collect();
    let act = thread.last_act().1;
    if let Some(edited) = all[..from].iter().rposition(|(_, _, e)| *e == Some(act)) {
        shown.insert(0, edited);
    }
    for index in shown {
        let (who, body, edited) = &all[index];
        let mark = if edited.is_some() { " [edited]" } else { "" };
        let mut body = body.lines();
        let first = body.next().unwrap_or_default();
        lines.push(format!("   {who}{mark}: {first}"));
        for more in body {
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
    Version(PathBuf, usize, u32),
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

    fn version(path: &Path, line: usize, found: u32) -> Self {
        Self {
            kind: ErrorKind::Version(path.to_path_buf(), line, found),
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
            ErrorKind::Version(path, line, found) => write!(
                f,
                "{AGENTS_FILE} line {line}: format version {found}, this build writes \
                 {FORMAT_VERSION}; delete {} to start over",
                path.display()
            ),
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
    use std::path::Path;

    use fathomable_testing::TempDir;
    use std::time::Duration;

    use crate::annotations::{Author, Draft, LineRange, Reply, Store, ThreadId};
    use fathomable_testing::vocabulary as test_vocab;

    use super::{Blob, Register, Subscriber, WatchWhen};

    type TestResult = Result<(), Box<dyn Error>>;

    /// A register of another format version is refused with the line,
    /// both versions, and the path to delete (ADR 0062).
    #[test]
    fn a_register_of_another_format_version_is_refused() -> TestResult {
        let dir = TempDir::new("agents-version")?;
        let path = dir.0.join("agents.jsonl");
        let stale = r#"{"event":"subscribe","v":0,"id":"s-1","kind":"coder","created":1}"#;
        fs::write(&path, format!("{stale}\n"))?;
        let error = Register::open(&path, 1_000, DAY)
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            error,
            Some(format!(
                "agents.jsonl line 1: format version 0, this build writes 1; delete {} to start over",
                path.display()
            ))
        );
        fs::write(
            &path,
            format!("{}\n", stale.replace(r#""v":0"#, r#""v":1"#)),
        )?;
        assert_eq!(Register::open(&path, 1_000, DAY)?.subscribers().len(), 1);
        Ok(())
    }

    const TEXT: &str = "one\ntwo\nthree\nfour\n";
    const DAY: Duration = Duration::from_hours(24);

    fn store_with_thread(
        dir: &TempDir,
    ) -> Result<(Store, crate::annotations::ThreadId), Box<dyn Error>> {
        let mut store = Store::open(dir.0.join("threads.jsonl"))?;
        let id = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(2, 3),
                "tighten this",
            ),
            TEXT,
            100,
        )?;
        Ok((store, id))
    }

    /// A thread is delivered once per act of the user's; an agent's
    /// reply, this session's or another's, ends the delivery until the
    /// user speaks again (ADR 0058).
    #[test]
    fn a_message_is_delivered_once_until_the_user_speaks() -> TestResult {
        let dir = TempDir::new("agents-once")?;
        let (mut store, id) = store_with_thread(&dir)?;
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", Some("bot"), None, 200)?;
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
        assert!(
            reg.deliverable(&sub, store.threads()).is_empty(),
            "another agent's answer is an answer"
        );
        store.reply(&id, Reply::new(Author::User, 204, "and?"))?;
        assert_eq!(reg.deliverable(&sub, store.threads()).len(), 1);
        // Reloading keeps the delivery and the subscription.
        let again = Register::open(dir.0.join("agents.jsonl"), 300, DAY)?;
        assert_eq!(again.subscribers().len(), 1);
        assert_eq!(again.deliverable(&sub, store.threads()).len(), 1);
        Ok(())
    }

    /// A subscription covers the whole workspace (ADR 0055): a thread on
    /// any file is deliverable while the user has the last word. A
    /// second `subscribe` refreshes the record and cannot change the type,
    /// and a register line written with a follow list still loads.
    #[test]
    fn a_subscription_covers_the_whole_workspace() -> TestResult {
        let dir = TempDir::new("agents-scope")?;
        let (mut store, id) = store_with_thread(&dir)?;
        let other = store.annotate(
            Draft::new(
                Author::User,
                Path::new("elsewhere/b.md"),
                LineRange::new(1, 1),
                "and this?",
            ),
            TEXT,
            150,
        )?;
        let path = dir.0.join("agents.jsonl");
        fs::write(
            &path,
            "{\"event\":\"subscribe\",\"v\":1,\"id\":\"s-1\",\"kind\":\"coder\",\"paths\":[\"a.md\"],\"created\":200}\n",
        )?;
        let mut reg = Register::open(&path, 200, DAY)?;
        let sub = reg.subscriber("s-1").ok_or("no subscriber")?.clone();
        let mut pending: Vec<&ThreadId> = reg
            .deliverable(&sub, store.threads())
            .iter()
            .map(|t| t.id())
            .collect();
        pending.sort_unstable();
        let mut both = vec![&id, &other];
        both.sort_unstable();
        assert_eq!(pending, both, "the old follow list is ignored");
        let me = Author::agent("bot").subscribed("s-1", "coder");
        store.reply(&id, Reply::new(me, 201, "I was here"))?;
        assert_eq!(
            reg.deliverable(&sub, store.threads()),
            [store.thread(&other).ok_or("gone")?]
        );
        store.reply(&id, Reply::new(Author::User, 202, "and?"))?;
        assert_eq!(reg.deliverable(&sub, store.threads()).len(), 2);
        reg.subscribe("s-1", "coder", Some("bot"), None, 203)?;
        assert_eq!(
            reg.subscriber("s-1").and_then(Subscriber::name),
            Some("bot")
        );
        assert_eq!(
            reg.subscribe("s-1", "reviewer", None, None, 204)
                .err()
                .map(|e| e.to_string()),
            Some("session s-1 is subscribed as coder; a type cannot change".to_owned())
        );
        Ok(())
    }

    #[test]
    fn nags_count_checks_and_expiry_drops_silent_sessions() -> TestResult {
        let dir = TempDir::new("agents-nag")?;
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", None, None, 200)?;
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
        let dir = TempDir::new("agents-watch")?;
        let (mut store, id) = store_with_thread(&dir)?;
        let other = store.annotate(
            Draft::new(
                Author::User,
                Path::new("a.md"),
                LineRange::new(1, 1),
                "also",
            ),
            TEXT,
            101,
        )?;
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", None, None, 200)?;
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
        store.resolve(&id, None, 207)?;
        let fired = reg.fire("s-1", store.threads(), 208)?;
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].watch.remind(), std::slice::from_ref(&other));
        assert!(reg.watches().is_empty());
        assert_eq!(reg.unwatch("s-1", &id, 209).ok(), None);
        Ok(())
    }

    /// A fired watch and the threads it reminds of are shown in full
    /// however tight the budget: the watch is spent when it fires, and a
    /// reminded thread need not be pending, so `threads` could
    /// never hand either of them over a second time.
    #[test]
    fn a_fired_watch_outranks_the_line_budget() -> TestResult {
        let dir = TempDir::new("agents-mustshow")?;
        let (mut store, id) = store_with_thread(&dir)?;
        let other = store.annotate(
            Draft::new(
                Author::User,
                Path::new("b.md"),
                LineRange::new(1, 4),
                "remind me",
            ),
            TEXT,
            160,
        )?;
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", Some("bot"), None, 200)?;
        reg.watch("s-1", &id, WatchWhen::Resolved, vec![other.clone()], 201)?;
        // The subscriber itself spoke last on the reminded thread, so it
        // is not pending and `deliverable` would never return it.
        store.reply(
            &other,
            Reply::new(Author::agent("bot").subscribed("s-1", "coder"), 202, "mine"),
        )?;
        store.resolve(&id, None, 203)?;
        let sub = reg.subscriber("s-1").ok_or("no subscriber")?.clone();
        let fired = reg.fire("s-1", store.threads(), 204)?;
        assert_eq!(fired.len(), 1);
        let remind: Vec<_> = store
            .threads()
            .iter()
            .filter(|t| t.id() == &other)
            .collect();
        assert_eq!(remind.len(), 1);
        let mut blob = Blob {
            fired: fired.into_iter().map(|f| (f, remind.clone())).collect(),
            ..Blob::default()
        };
        blob.fit(&sub, 1);
        assert!(blob.listed.is_empty(), "a fired watch was listed away");
        let text = blob.render(&sub, "user");
        assert!(text.contains("watch fired:"), "{text}");
        assert!(text.contains(&format!("── thread {other} ")), "{text}");
        assert!(!text.contains("more; call `threads`"), "{text}");
        Ok(())
    }

    /// A budget too small for even one thread still shows one. Listing
    /// every thread would record no delivery, so the identical id-only
    /// blob would come back at every turn-end for good.
    #[test]
    fn a_budget_smaller_than_one_thread_still_makes_progress() -> TestResult {
        let dir = TempDir::new("agents-tiny")?;
        let (store, _) = store_with_thread(&dir)?;
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", Some("bot"), None, 200)?;
        let sub = reg.subscriber("s-1").ok_or("no subscriber")?.clone();
        let mut blob = Blob {
            fresh: reg.deliverable(&sub, store.threads()),
            ..Blob::default()
        };
        assert!(!blob.fresh.is_empty());
        blob.fit(&sub, 1);
        assert_eq!(blob.shown().count(), 1, "the blob showed nothing");
        Ok(())
    }

    /// A comment reaches the agent whole, however many lines it has:
    /// the sixth point of a six-point comment is as much an instruction
    /// as the first, and nothing marks a cut the agent cannot see.
    #[test]
    fn a_long_message_is_delivered_whole() -> TestResult {
        let dir = TempDir::new("agents-whole")?;
        let (mut store, id) = store_with_thread(&dir)?;
        let points: Vec<String> = (1..=6).map(|n| format!("{n}. point {n}")).collect();
        store.reply(&id, Reply::new(Author::User, 150, points.join("\n")))?;
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", Some("bot"), None, 200)?;
        let sub = reg.subscriber("s-1").ok_or("no subscriber")?.clone();
        let mut blob = Blob {
            fresh: reg.deliverable(&sub, store.threads()),
            ..Blob::default()
        };
        blob.fit(&sub, 40);
        let text = blob.render(&sub, "user");
        assert!(text.contains("   user: 1. point 1"), "{text}");
        for point in &points[1..] {
            assert!(text.contains(&format!("     {point}")), "{text}");
        }
        Ok(())
    }

    #[test]
    fn the_blob_is_bounded_and_says_what_to_call() -> TestResult {
        let dir = TempDir::new("agents-blob")?;
        let (mut store, id) = store_with_thread(&dir)?;
        store.reply(
            &id,
            Reply::new(
                Author::agent("x").subscribed("s-9", "reviewer"),
                150,
                "hm\nsecond line",
            ),
        )?;
        store.reply(&id, Reply::new(Author::User, 151, "and?"))?;
        for n in 0..5 {
            store.annotate(
                Draft::new(
                    Author::User,
                    Path::new("b.md"),
                    LineRange::new(1, 4),
                    format!("note {n}"),
                ),
                TEXT,
                160 + n,
            )?;
        }
        let mut reg = Register::open(dir.0.join("agents.jsonl"), 200, DAY)?;
        reg.subscribe("s-1", "coder", Some("bot"), None, 200)?;
        let sub = reg.subscriber("s-1").ok_or("no subscriber")?.clone();
        let fresh = reg.deliverable(&sub, store.threads());
        let mut blob = Blob {
            fresh,
            ..Blob::default()
        };
        blob.fit(&sub, 20);
        // The header still counts every thread that needs a reply, the
        // ones named by id alone included.
        let text = blob.render(&sub, "user");
        assert!(
            text.starts_with("FATHOMABLE: 6 review threads need your reply (you are bot (coder)).")
        );
        assert!(text.contains("thread_reply"));
        assert!(text.contains("x (reviewer): hm"));
        assert!(text.contains("     second line"));
        assert!(text.contains("more; call `threads`"));
        assert!(text.lines().count() <= 20 + 6, "{text}");
        // Every tool or parameter the blob names is one the vocabulary
        // knows, which the fathomable crate checks against the schema.
        for ident in test_vocab::idents(&text) {
            assert!(test_vocab::is_known(ident), "blob names unknown `{ident}`");
        }
        assert!(
            blob.shown().count() + blob.listed.len() == 6,
            "every thread is either shown or listed"
        );
        let empty = Blob::default();
        assert!(empty.is_empty());
        assert_eq!(empty.render(&sub, "user"), "");
        let reminder = Blob {
            reminder: store.threads().iter().take(2).collect(),
            ..Blob::default()
        };
        let text = reminder.render(&sub, "user");
        assert_eq!(text.lines().count(), 1);
        assert!(text.contains("2 threads still unanswered: a.md:2-3, b.md:1-4"));
        Ok(())
    }
}
