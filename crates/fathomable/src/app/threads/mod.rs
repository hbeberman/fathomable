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
//! lines (`open`), git reach, re-anchoring, lifecycle, and summary facts.
//! All of it is plain state, tested without a terminal (ADRs 0013, 0046,
//! and 0086).

pub(crate) mod cursor;
pub(crate) mod delete;
pub(crate) mod detached;
pub(crate) mod draft;
pub(crate) mod file;
pub(crate) mod fold;
pub(crate) mod list;
pub(crate) mod list_fold;
pub(crate) mod open;
pub(crate) mod pane;
pub(crate) mod proposed;
pub(crate) mod reach;
pub(crate) mod reanchor;
pub(crate) mod stubs;
pub(crate) mod summary;
pub(crate) mod words;

pub(crate) use draft::{Compose, ComposeTarget};

use std::path::{Path, PathBuf};

use fathomable_core::annotations::{
    AgentReplyCommand, Author, Draft, Lifecycle, LineHashes, LineRange, MessageTarget, Placement,
    ResolutionOutcome, Status, Store, Thread, ThreadId,
};
use fathomable_core::clock::now;
use fathomable_core::reanchor::{Mapping, map_range};

use crate::app::App;
use crate::app::input::bindings::Action;
use crate::app::threads::words::Words;

/// A thread's status, which is its colour in the gutter, the file-threads
/// pane, and the review list (ADR 0039); where the thread is placed is
/// [`Mark::placement`]. Ordered by urgency, so the most urgent of several
/// on one row is their `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ThreadState {
    Resolved,
    Active,
    Proposed,
}

impl ThreadState {
    pub(super) fn of(thread: &Thread) -> Self {
        match thread.lifecycle() {
            Lifecycle::Active => Self::Active,
            Lifecycle::ResolutionProposed => Self::Proposed,
            Lifecycle::Resolved => Self::Resolved,
        }
    }
}

/// One thread placed in the current text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mark {
    id: ThreadId,
    placement: Placement,
    words: Words,
}

impl Mark {
    pub(crate) fn id(&self) -> &ThreadId {
        &self.id
    }

    /// The lines the thread is drawn at; `None` for a thread on the file
    /// as a whole (ADR 0063).
    pub(crate) fn range(&self) -> Option<LineRange> {
        self.placement.range()
    }

    /// Whether the thread's lines overlap `lines`; a thread on the file
    /// as a whole covers none.
    pub(crate) fn covers(&self, lines: LineRange) -> bool {
        self.range().is_some_and(|range| overlaps(range, lines))
    }

    pub(crate) fn kind(&self) -> ThreadState {
        self.words.state()
    }

    /// The thread's words and circle (ADR 0032, ADR 0066).
    pub(crate) fn words(&self) -> Words {
        self.words
    }

    /// The one circle every surface draws for the thread (ADR 0066).
    pub(crate) fn glyph(&self) -> &'static str {
        self.words.glyph()
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

/// How a message's author reads on a row: the user by the configured
/// name (ADR 0058), or an agent by its name.
pub(crate) fn author_label(author: &Author, user: &str) -> String {
    match author {
        Author::User => user.to_owned(),
        Author::Agent { name, .. } => name.clone(),
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
    pub(super) fn thread_store_unavailable(&self) -> String {
        self.thread_store_error.as_ref().map_or_else(
            || "threads unavailable".to_owned(),
            |error| format!("threads unavailable; :status: {error}"),
        )
    }

    /// The store, or a status-line notice explaining why there is none.
    pub(super) fn store_mut(&mut self) -> Option<&mut Store> {
        if self.store.is_none() {
            self.error(self.thread_store_unavailable());
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
        self.marks().iter().filter(|mark| {
            matches!(
                mark.placement,
                Placement::Anchored(_) | Placement::Edited(_)
            )
        })
    }

    /// The most urgent mark overlapping `lines` (a rendered row can carry
    /// several source lines).
    pub(crate) fn mark_in(&self, lines: LineRange) -> Option<ThreadState> {
        self.placed_marks()
            .filter(|mark| mark.covers(lines))
            .map(Mark::kind)
            .max()
    }

    /// `(unresolved, total)` threads on the current document.
    pub(crate) fn thread_counts(&self) -> (usize, usize) {
        let unresolved = self
            .marks()
            .iter()
            .filter(|mark| matches!(mark.kind(), ThreadState::Active | ThreadState::Proposed))
            .count();
        (unresolved, self.marks().len())
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
        let hashes = LineHashes::of(&text);
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
            .filter(|thread| thread.locate_in(&hashes).is_detached())
            .filter_map(|thread| {
                // Where it sat in the old text; a thread already detached
                // there has nothing to follow.
                previous
                    .iter()
                    .find(|(id, _)| id == thread.id())
                    .filter(|(_, placement)| !placement.is_detached())
                    .and_then(|(id, placement)| Some((id.clone(), placement.range()?)))
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
        self.reconcile_agent_activity();
        self.refresh_marks(index);
    }

    pub(super) fn refresh_marks(&mut self, index: usize) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let Some(doc) = self.docs.get_mut(index) else {
            return;
        };
        let hashes = LineHashes::of(doc.document.text().unwrap_or_default());
        doc.marks = store
            .for_path(&doc.relative)
            .filter(|thread| self.reach.here(thread))
            .map(|thread| {
                let placement = thread.locate_in(&hashes);
                Mark {
                    id: thread.id().clone(),
                    placement,
                    words: Words::of(Some(placement), thread),
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
        self.reconcile_agent_activity();
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
        // A thread on the file as a whole has no start and comes first.
        marks.sort_by_key(|mark| mark.range().map(|range| range.start()));
        marks.into_iter().map(|mark| mark.id().clone()).collect()
    }

    /// Every thread on the work in the order `L` / `H` walk: files by
    /// path, threads by line. The open file contributes its marks, so
    /// re-anchored ranges keep their place.
    pub(super) fn workspace_threads(&self) -> Vec<ThreadId> {
        let current = self.current.map(|i| self.docs[i].relative.as_path());
        let mut others: Vec<(&Path, Option<usize>, ThreadId)> = self
            .store
            .iter()
            .flat_map(Store::threads)
            .filter(|thread| self.reach.here(thread) && Some(thread.path()) != current)
            .map(|thread| {
                let start = thread.range().map(|range| range.start());
                (thread.path(), start, thread.id().clone())
            })
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
            .filter(|mark| mark.covers(lines))
            .map(|mark| mark.id().clone())
            .collect()
    }

    /// Apply an agent reply and its lifecycle effects as one core operation.
    #[expect(
        clippy::too_many_arguments,
        reason = "Keep the socket request fields explicit at the viewer boundary."
    )]
    pub(super) fn agent_reply(
        &mut self,
        id: &ThreadId,
        author: Author,
        body: String,
        caller: String,
        resolve: bool,
        lines: Option<LineRange>,
        idempotency_key: Option<String>,
    ) -> Result<(Thread, ResolutionOutcome, bool), String> {
        let root = self.workspace.root().to_path_buf();
        let head = self.workspace.head_commit();
        let when = now();
        let mut command = AgentReplyCommand::new(author, when, body).at_head(head);
        if resolve {
            command = command.resolve();
        }
        if let Some(lines) = lines {
            command = command.relocate(lines);
        }
        if let Some(key) = idempotency_key {
            command = command.idempotent(caller, key);
        }
        let outcome = {
            let Some(store) = self.store.as_mut() else {
                return Err(self.thread_store_unavailable());
            };
            store.agent_reply(id, command, |path| {
                std::fs::read_to_string(root.join(path)).map_err(|error| {
                    fathomable_core::annotations::StoreError::message(format!(
                        "cannot read {}: {error}",
                        path.display()
                    ))
                })
            })
        };
        self.reconcile_agent_activity();
        let outcome = outcome.map_err(|error| error.to_string())?;
        let resolution = *outcome.value();
        let replayed = outcome.replayed();
        tracing::info!(%id, ?resolution, replayed, "agent reply added");
        for index in 0..self.docs.len() {
            self.refresh_marks(index);
        }
        // Answered with the thread as it now stands (ADR 0055).
        self.store
            .as_ref()
            .and_then(|store| store.thread(id))
            .cloned()
            .map(|thread| (thread, resolution, replayed))
            .ok_or_else(|| format!("thread {id} vanished after the reply"))
    }

    /// An agent starts a thread on `range` of `path` (ADR 0061), or on
    /// the file as a whole with no range (ADR 0063): the comment is
    /// stamped with `HEAD` as the user's is, the viewer toasts it and
    /// refreshes its marks, and nothing moves or is marked seen. Answers
    /// with the thread as it stands.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "The caller scope follows the owned socket request into this handler."
    )]
    pub(super) fn agent_start(
        &mut self,
        path: &Path,
        range: Option<LineRange>,
        author: Author,
        body: String,
        caller: String,
        idempotency_key: Option<String>,
    ) -> Result<(Thread, bool), String> {
        let label = author.to_string();
        let place = match range {
            Some(range) => format!("{}:{}", path.display(), range.start()),
            None => path.display().to_string(),
        };
        let draft = match range {
            Some(range) => Draft::new(author, path, range, body),
            None => Draft::on_file(author, path, body),
        }
        .at_commit(self.workspace.head_commit());
        let result = if let Some(key) = idempotency_key {
            let root = self.workspace.root().to_path_buf();
            let Some(store) = self.store.as_mut() else {
                return Err(self.thread_store_unavailable());
            };
            store
                .annotate_idempotent_for_caller(draft, now(), &caller, &key, |path| {
                    std::fs::read_to_string(root.join(path)).map_err(|error| {
                        fathomable_core::annotations::StoreError::message(format!(
                            "cannot read {}: {error}",
                            path.display()
                        ))
                    })
                })
                .map(|outcome| {
                    let replayed = outcome.replayed();
                    (outcome.into_value(), replayed)
                })
        } else {
            let text = std::fs::read_to_string(self.workspace.root().join(path))
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            let Some(store) = self.store.as_mut() else {
                return Err(self.thread_store_unavailable());
            };
            store.annotate(draft, &text, now()).map(|id| (id, false))
        };
        self.reconcile_agent_activity();
        let (id, replayed) = result.map_err(|error| error.to_string())?;
        tracing::info!(%id, %place, %label, "agent thread started");
        self.refresh_reach();
        self.refresh_all_marks();
        self.store
            .as_ref()
            .and_then(|store| store.thread(&id))
            .cloned()
            .map(|thread| (thread, replayed))
            .ok_or_else(|| format!("thread {id} vanished after the comment"))
    }

    /// Move the cursor to the first line of `id`, when the document has it.
    pub(super) fn goto_thread(&mut self, id: &ThreadId) {
        let Some(mark) = self.marks().iter().find(|mark| mark.id() == id) else {
            // A past thread has no mark: its stored lines are the
            // nearest thing to it (ADR 0072).
            if let Some(range) = self
                .thread(id)
                .filter(|thread| self.reach.past(thread))
                .and_then(fathomable_core::annotations::Thread::range)
            {
                self.view_mut().goto_source_line(range.start());
            }
            return;
        };
        match mark.placement() {
            Placement::Detached(_) => {
                let anchor = self.detached_anchor(mark);
                self.view_mut().goto_detached_row(anchor);
            }
            Placement::Anchored(range) | Placement::Edited(range) => {
                self.view_mut().goto_source_line(range.start());
            }
            // Its block stands above the first line (ADR 0063).
            Placement::File => self.view_mut().goto_row(0),
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

    /// Resolve `id` when open, fixing it to `HEAD` (ADR 0072), reopen it
    /// otherwise; every loaded document's marks follow.
    pub(super) fn toggle_resolved(&mut self, id: &ThreadId) {
        let head = self.workspace.head_commit();
        let Some(store) = self.store_mut() else {
            return;
        };
        let open = store.thread(id).is_some_and(|t| t.status() == Status::Open);
        let result = if open {
            store.resolve(id, head.as_deref(), now())
        } else {
            store.reopen(id, now())
        };
        self.reconcile_agent_activity();
        match result {
            Ok(()) => {
                tracing::info!(%id, resolved = open, "thread status changed");
                self.refresh_all_marks();
                self.notice(if open { "resolved" } else { "reopened" });
            }
            Err(error) => self.notice(format!("cannot update thread: {error}")),
        }
    }

    /// Toggle one-shot auto-resolve for an unresolved thread.
    pub(super) fn toggle_auto_resolve(&mut self, id: &ThreadId) {
        if self
            .thread(id)
            .is_some_and(|thread| thread.lifecycle() == Lifecycle::Resolved)
        {
            self.notice("auto-resolve is unavailable on a resolved thread");
            return;
        }
        let Some(store) = self.store_mut() else {
            return;
        };
        let result = store.toggle_auto_resolve(id, now());
        self.reconcile_agent_activity();
        match result {
            Ok(value) => {
                tracing::info!(%id, ?value, "thread auto-resolve changed");
                self.refresh_all_marks();
                self.notice(if value.is_enabled() {
                    "auto-resolve enabled"
                } else {
                    "auto-resolve disabled"
                });
            }
            Err(error) => self.notice(format!("cannot update thread: {error}")),
        }
    }

    /// Run a direct header action against the row's explicit thread.
    pub(crate) fn thread_summary_action(&mut self, id: &ThreadId, action: Action) {
        let place = self.review_selected_index();
        match action {
            Action::ToggleAutoResolve => self.toggle_auto_resolve(id),
            Action::ToggleResolved => self.toggle_resolved(id),
            _ => {}
        }
        self.review_reselect(place);
    }
}

#[cfg(test)]
mod tests;
