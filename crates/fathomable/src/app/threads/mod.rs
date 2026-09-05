// @okf-doc: /decisions/0013-annotation-storage-and-ux.md
//! Threads on top of the view: the store, the marks, the draft, and
//! the stubs, with the thread concept's other modules beneath.
//!
//! [`App`] keeps the [`Store`] for the workspace; every open document
//! carries the [`Mark`]s of its threads, re-located whenever the text
//! changes. The draft ([`Compose`], `draft`) starts a thread, replies to
//! one, or edits a message, written in the thread's rows; the stubs
//! (`stubs`) read a thread in place. The submodules
//! are the thread cursor (`cursor`), the threads pane (`pane`),
//! the review list (`list`), deletion, detached rows, the open thread's
//! lines (`open`), git reach, re-anchoring, waiting threads, and the
//! placement and state words. All of it is plain state, tested without a
//! terminal (ADRs 0013 and 0046).

pub(crate) mod cursor;
pub(crate) mod delete;
pub(crate) mod detached;
pub(crate) mod draft;
pub(crate) mod list;
pub(crate) mod open;
pub(crate) mod pane;
pub(crate) mod proposed;
pub(crate) mod reach;
pub(crate) mod reanchor;
pub(crate) mod stubs;
pub(crate) mod waiting;
pub(crate) mod words;

pub(crate) use draft::{Compose, ComposeTarget};

use std::path::{Path, PathBuf};

use fathomable_core::annotations::{
    Author, Draft, LineRange, MessageTarget, Placement, Reply, Status, Store, Thread, ThreadId,
};
use fathomable_core::clock::now;
use fathomable_core::reanchor::{Mapping, map_range};

use crate::app::App;

/// A thread's status, which is its colour in the gutter, the file-threads
/// pane, and the review list (ADR 0039); where the thread is placed is
/// [`Mark::placement`]. Ordered by urgency, so the most urgent of several
/// on one row is their `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ThreadState {
    Resolved,
    Open,
    /// Open, and an agent has the last word (ADR 0030, ADR 0058).
    Waiting,
}

impl ThreadState {
    pub(super) fn of(thread: &Thread) -> Self {
        if thread.awaits_user() {
            return Self::Waiting;
        }
        match thread.status() {
            Status::Open => Self::Open,
            Status::Resolved => Self::Resolved,
        }
    }
}

/// One thread placed in the current text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mark {
    id: ThreadId,
    placement: Placement,
    kind: ThreadState,
}

impl Mark {
    pub(crate) fn id(&self) -> &ThreadId {
        &self.id
    }

    pub(crate) fn range(&self) -> LineRange {
        self.placement.range()
    }

    pub(crate) fn kind(&self) -> ThreadState {
        self.kind
    }

    /// Where the thread sits in the current text.
    pub(crate) fn placement(&self) -> Placement {
        self.placement
    }

    /// Whether the annotated lines are gone: the thread is shown on a
    /// row of its own (ADR 0039), not at its last known range.
    pub(crate) fn is_detached(&self) -> bool {
        self.placement.is_detached()
    }
}

pub(super) fn overlaps(a: LineRange, b: LineRange) -> bool {
    a.start() <= b.end() && b.start() <= a.end()
}

/// Convert a zero-based message position into its storage target.
pub(super) fn message_target(message: usize) -> MessageTarget {
    if message == 0 {
        MessageTarget::Comment
    } else {
        MessageTarget::Reply(message - 1)
    }
}

impl App {
    /// The store, or a status-line notice explaining why there is none.
    pub(super) fn store_mut(&mut self) -> Option<&mut Store> {
        if self.store.is_none() {
            self.notice("threads unavailable; see the log");
        }
        self.store.as_mut()
    }

    /// The thread with `id`, if the store has it.
    pub(crate) fn thread(&self, id: &ThreadId) -> Option<&Thread> {
        self.store.as_ref()?.thread(id)
    }

    /// Marks of the current document, oldest thread first.
    pub(crate) fn marks(&self) -> &[Mark] {
        self.current
            .and_then(|i| self.docs.get(i))
            .map_or(&[], |doc| doc.marks.as_slice())
    }

    /// The marks placed on lines, leaving out the detached ones, which
    /// draw on rows of their own (ADR 0039).
    pub(super) fn placed_marks(&self) -> impl Iterator<Item = &Mark> {
        self.marks().iter().filter(|mark| !mark.is_detached())
    }

    /// The most urgent mark overlapping `lines` (a rendered row can carry
    /// several source lines).
    pub(crate) fn mark_in(&self, lines: LineRange) -> Option<ThreadState> {
        self.placed_marks()
            .filter(|mark| overlaps(mark.range(), lines))
            .map(Mark::kind)
            .max()
    }

    /// `(open, total)` threads on the current document.
    pub(crate) fn thread_counts(&self) -> (usize, usize) {
        let open = self
            .marks()
            .iter()
            .filter(|mark| matches!(mark.kind(), ThreadState::Open | ThreadState::Waiting))
            .count();
        (open, self.marks().len())
    }

    /// Re-locate every thread of the document at `index` in its text.
    /// After a reload: threads whose lines stopped matching are followed
    /// through the diff from `old` to the new text and, when their lines
    /// were rewritten rather than removed, re-anchored onto the
    /// replacement and recorded as edited (ADR 0019).
    pub(super) fn remap_marks(&mut self, index: usize, old: &str) {
        let Some(doc) = self.docs.get(index) else {
            return;
        };
        let text = doc.document.text().unwrap_or_default().to_owned();
        let path = doc.relative.clone();
        let previous: Vec<(ThreadId, Placement)> = doc
            .marks
            .iter()
            .map(|mark| (mark.id.clone(), mark.placement))
            .collect();
        let Some(store) = self.store.as_mut() else {
            return;
        };
        let stale: Vec<(ThreadId, LineRange)> = store
            .for_path(&path)
            .filter(|thread| thread.locate(&text).is_detached())
            .filter_map(|thread| {
                // Where it sat in the old text; a thread already detached
                // there has nothing to follow.
                previous
                    .iter()
                    .find(|(id, _)| id == thread.id())
                    .filter(|(_, placement)| !placement.is_detached())
                    .map(|(id, placement)| (id.clone(), placement.range()))
            })
            .collect();
        for (id, range) in stale {
            let target = match map_range(old, &text, range) {
                Mapping::Edited(range) | Mapping::Moved(range) => range,
                Mapping::Removed => continue,
            };
            match store.relocate(&id, target, &text, now()) {
                Ok(()) => {
                    tracing::info!(%id, path = %path.display(), from = %range, to = %target, "thread re-anchored to edited lines");
                }
                Err(error) => tracing::warn!(%id, %error, "cannot re-anchor thread"),
            }
        }
        self.refresh_marks(index);
    }

    pub(super) fn refresh_marks(&mut self, index: usize) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let Some(doc) = self.docs.get_mut(index) else {
            return;
        };
        let text = doc.document.text().unwrap_or_default();
        doc.marks = store
            .for_path(&doc.relative)
            .filter(|thread| self.reach.includes(thread))
            .map(|thread| {
                let placement = thread.locate(text);
                Mark {
                    id: thread.id().clone(),
                    placement,
                    kind: ThreadState::of(thread),
                }
            })
            .collect();
        let detached = doc.marks.iter().filter(|m| m.is_detached()).count();
        tracing::debug!(path = %doc.relative.display(), marks = doc.marks.len(), detached, "marks refreshed");
        // The threads pane's height follows the marks (ADR 0027), and
        // the detached ones take rows of their own (ADR 0039).
        if self.current == Some(index) {
            self.place_detached_rows();
            self.place_stub_rows();
            self.scroll_tree();
        }
    }

    /// Move every thread whose path `moved` maps to its new path, after a
    /// file or directory rename (ADR 0028). Range and anchor are kept, so
    /// the threads sit on the same lines in the renamed file.
    pub(super) fn move_threads(&mut self, moved: &impl Fn(&Path) -> Option<PathBuf>) {
        let Some(store) = self.store.as_mut() else {
            return;
        };
        let targets: Vec<(ThreadId, PathBuf)> = store
            .threads()
            .iter()
            .filter_map(|thread| Some((thread.id().clone(), moved(thread.path())?)))
            .collect();
        if targets.is_empty() {
            return;
        }
        let when = now();
        for (id, path) in &targets {
            match store.move_path(id, path, when) {
                Ok(()) => tracing::info!(%id, to = %path.display(), "thread moved with its file"),
                Err(error) => tracing::warn!(%id, %error, "cannot move thread"),
            }
        }
        self.refresh_all_marks();
    }

    /// Re-locate every loaded document's threads, after a change that
    /// may touch files other than the current one (ADR 0025).
    pub(super) fn refresh_all_marks(&mut self) {
        for index in 0..self.docs.len() {
            self.refresh_marks(index);
        }
    }

    /// The document's threads in line order: by first line, then the
    /// order the store holds them (ADR 0027). This is the order `]c` / `[c`
    /// in the text and `j` / `k` in the threads pane walk.
    pub(crate) fn file_threads(&self) -> Vec<ThreadId> {
        let mut marks: Vec<&Mark> = self.marks().iter().collect();
        marks.sort_by_key(|mark| mark.range().start());
        marks.into_iter().map(|mark| mark.id().clone()).collect()
    }

    /// Every thread on the work in the order `L` / `H` walk: files by
    /// path, threads by line. The open file contributes its marks, so
    /// re-anchored ranges keep their place.
    pub(super) fn workspace_threads(&self) -> Vec<ThreadId> {
        let current = self.current.map(|i| self.docs[i].relative.as_path());
        let mut others: Vec<(&Path, usize, ThreadId)> = self
            .store
            .iter()
            .flat_map(Store::threads)
            .filter(|thread| self.reach.includes(thread) && Some(thread.path()) != current)
            .map(|thread| (thread.path(), thread.range().start(), thread.id().clone()))
            .collect();
        others.sort();
        let mut order = Vec::with_capacity(others.len() + self.marks().len());
        let mut here = Some(self.file_threads());
        for (path, _, id) in others {
            if current.is_some_and(|cur| path > cur)
                && let Some(here) = here.take()
            {
                order.extend(here);
            }
            order.push(id);
        }
        order.extend(here.into_iter().flatten());
        order
    }

    #[cfg(test)]
    /// `(current, total)`, 1-based, of the cursor's thread among the
    /// file's, for the pane header.
    pub(crate) fn thread_position(&self) -> Option<(usize, usize)> {
        let cursor = self.thread_cursor();
        let id = cursor.thread()?;
        let order = self.file_threads();
        let index = order.iter().position(|other| other == id)?;
        Some((index + 1, order.len()))
    }

    #[cfg(test)]
    /// `(current, total)`, 1-based, of the cursor's thread among the
    /// workspace's, for the pane header.
    pub(crate) fn thread_position_across(&self) -> Option<(usize, usize)> {
        let cursor = self.thread_cursor();
        let id = cursor.thread()?;
        let order = self.workspace_threads();
        let index = order.iter().position(|other| other == id)?;
        Some((index + 1, order.len()))
    }

    /// Threads whose range touches the cursor's rendered row, or the
    /// detached threads standing on it (ADR 0039).
    pub(crate) fn threads_at_cursor(&self) -> Vec<ThreadId> {
        let view = self.view();
        // On a thread's stub or expanded rows, that thread (ADR 0049).
        if let Some((stub, _, _)) = self.stub_on_row(view.cursor().row)
            && let Some(id) = stub.thread()
        {
            return vec![id.clone()];
        }
        if let Some(anchor) = view.detached_anchor_of_row(view.cursor().row) {
            return self
                .detached_marks_at(anchor)
                .map(|mark| mark.id().clone())
                .collect();
        }
        let lines = view.source_lines_of_row(view.cursor().row).or_else(|| {
            view.cursor_source_line()
                .map(|line| LineRange::new(line, line))
        });
        let Some(lines) = lines else {
            return Vec::new();
        };
        self.placed_marks()
            .filter(|mark| overlaps(mark.range(), lines))
            .map(|mark| mark.id().clone())
            .collect()
    }

    /// A reply arriving over the socket (ADR 0014), optionally proposing
    /// that the thread be resolved (ADR 0053); the thread stays open either
    /// way, and the open panel is refreshed when it shows that thread.
    pub(super) fn agent_reply(
        &mut self,
        id: &ThreadId,
        author: Author,
        body: String,
        resolve: bool,
        lines: Option<LineRange>,
    ) -> Result<Thread, String> {
        let root = self.workspace.root().to_path_buf();
        let store = self
            .store
            .as_mut()
            .ok_or("threads unavailable; see the log")?;
        if store.thread(id).is_none() {
            return Err(format!("unknown thread {id}"));
        }
        let when = now();
        if let Some(lines) = lines {
            crate::app::threads::open::follow_reply_lines(store, &root, id, lines, when)?;
        }
        let reply = Reply::new(author, when, body);
        let reply = if resolve {
            reply.proposing_resolution()
        } else {
            reply
        };
        store.reply(id, reply).map_err(|e| e.to_string())?;
        tracing::info!(%id, proposes = resolve, "agent reply added");
        // The toast a store reload would raise (ADR 0030), for the viewer
        // the reply came through; a proposing reply says so (ADR 0053).
        if let Some(thread) = store.thread(id) {
            let place = format!("{}:{}", thread.path().display(), thread.range().start());
            self.push_toast(if resolve {
                format!("reply on {place}, proposes resolving")
            } else {
                format!("reply on {place}")
            });
        }
        for index in 0..self.docs.len() {
            self.refresh_marks(index);
        }
        // Answered with the thread as it now stands (ADR 0055).
        self.store
            .as_ref()
            .and_then(|store| store.thread(id))
            .cloned()
            .ok_or_else(|| format!("thread {id} vanished after the reply"))
    }

    /// An agent starts a thread on `range` of `path` (ADR 0061): the
    /// comment is stamped with `HEAD` as the user's is, the viewer
    /// toasts it and refreshes its marks, and nothing moves or is marked
    /// seen. Answers with the thread as it stands.
    pub(super) fn agent_start(
        &mut self,
        path: &Path,
        range: LineRange,
        author: Author,
        body: String,
    ) -> Result<Thread, String> {
        let text = std::fs::read_to_string(self.workspace.root().join(path))
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let label = author.to_string();
        let draft = Draft::new(author, path, range, body).at_commit(self.workspace.head_commit());
        let store = self
            .store
            .as_mut()
            .ok_or("threads unavailable; see the log")?;
        let id = store
            .annotate(draft, &text, now())
            .map_err(|e| e.to_string())?;
        tracing::info!(%id, path = %path.display(), %range, %label, "agent thread started");
        self.refresh_reach();
        self.refresh_all_marks();
        self.push_toast(format!(
            "comment on {}:{} from {label}",
            path.display(),
            range.start()
        ));
        self.store
            .as_ref()
            .and_then(|store| store.thread(&id))
            .cloned()
            .ok_or_else(|| format!("thread {id} vanished after the comment"))
    }

    /// Move the cursor to the first line of `id`, when the document has it.
    pub(super) fn goto_thread(&mut self, id: &ThreadId) {
        let Some(mark) = self.marks().iter().find(|mark| mark.id() == id) else {
            return;
        };
        if mark.is_detached() {
            let anchor = self.detached_anchor(mark);
            self.view_mut().goto_detached_row(anchor);
        } else {
            let line = mark.range().start();
            self.view_mut().goto_source_line(line);
        }
    }

    pub(super) fn message_for(&self, id: &ThreadId, target: MessageTarget) -> Option<(&str, bool)> {
        let thread = self.thread(id)?;
        match target {
            MessageTarget::Comment => Some((thread.comment(), thread.author().is_user())),
            MessageTarget::Reply(index) => thread
                .replies()
                .get(index)
                .map(|reply| (reply.body(), reply.author().is_user())),
        }
    }

    /// Resolve `id` when open, reopen it otherwise; every loaded
    /// document's marks follow.
    pub(super) fn toggle_resolved(&mut self, id: &ThreadId) {
        let Some(store) = self.store_mut() else {
            return;
        };
        let open = store.thread(id).is_some_and(|t| t.status() == Status::Open);
        let result = if open {
            store.resolve(id, now())
        } else {
            store.reopen(id, now())
        };
        match result {
            Ok(()) => {
                tracing::info!(%id, resolved = open, "thread status changed");
                self.refresh_all_marks();
                self.notice(if open { "resolved" } else { "reopened" });
            }
            Err(error) => self.notice(format!("cannot update thread: {error}")),
        }
    }
}

#[cfg(test)]
mod tests;
