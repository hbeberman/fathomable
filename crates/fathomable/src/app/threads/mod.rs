// @okf-doc: /decisions/0013-annotation-storage-and-ux.md
//! Threads on top of the view: the store, the marks, the comment box, and
//! the stubs, with the thread concept's other modules beneath.
//!
//! [`App`] keeps the [`Store`] for the workspace; every open document
//! carries the [`Mark`]s of its threads, re-located whenever the text
//! changes. The comment box ([`Compose`]) starts a thread or replies to
//! one; the stubs (`stubs`) read a thread in place. The submodules
//! are the thread cursor (`cursor`), the threads pane (`pane`),
//! the review list (`list`), deletion, detached rows, the open thread's
//! lines (`open`), git reach, re-anchoring, waiting threads, and the
//! placement and state words. All of it is plain state, tested without a
//! terminal (ADRs 0013 and 0046).

pub(crate) mod cursor;
pub(crate) mod delete;
pub(crate) mod detached;
pub(crate) mod list;
pub(crate) mod open;
pub(crate) mod pane;
pub(crate) mod reach;
pub(crate) mod reanchor;
pub(crate) mod stubs;
pub(crate) mod waiting;
pub(crate) mod words;

use std::path::{Path, PathBuf};

use fathomable_core::annotations::{
    Author, Draft, LineRange, MessageTarget, Party, Placement, Reply, Status, Store, Thread,
    ThreadId,
};
use fathomable_core::clock::now;
use fathomable_core::content::Content;
use fathomable_core::editor::{Buffer, Cell, Edit};
use fathomable_core::reanchor::{Mapping, map_range};

use crate::app::{App, Focus, Popup};

/// A thread's status, which is its colour in the gutter, the file-threads
/// pane, and the review list (ADR 0039); where the thread is placed is
/// [`Mark::placement`]. Ordered by urgency, so the most urgent of several
/// on one row is their `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ThreadState {
    Resolved,
    AutoResolved,
    Open,
    /// Open, and an agent wrote the newest message (ADR 0030).
    Waiting,
}

impl ThreadState {
    pub(super) fn of(thread: &Thread) -> Self {
        if thread.awaits(Party::User) {
            return Self::Waiting;
        }
        match thread.status() {
            Status::Open => Self::Open,
            Status::Resolved => Self::Resolved,
            Status::AutoResolved => Self::AutoResolved,
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

/// What the comment box will produce on submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComposeTarget {
    /// A new thread on these source lines.
    New(LineRange),
    /// A reply to an existing thread.
    Reply(ThreadId),
    /// A replacement for one user-authored message.
    Edit {
        thread: ThreadId,
        message: MessageTarget,
    },
}

/// The multi-line comment box (ADR 0005).
#[derive(Debug)]
pub(crate) struct Compose {
    target: ComposeTarget,
    buffer: Buffer,
    /// The text the box opened with, empty for a new comment or reply.
    original: String,
    /// Esc was pressed on a non-empty draft; the next Esc discards it.
    confirm_discard: bool,
}

impl Compose {
    pub(crate) fn target(&self) -> &ComposeTarget {
        &self.target
    }

    /// The draft and its cursor (ADR 0018).
    pub(crate) fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Whether the box is asking for a second Esc.
    pub(crate) fn confirming_discard(&self) -> bool {
        self.confirm_discard
    }

    /// A mouse click at a wrapped cell moves the cursor there.
    pub(crate) fn place_cursor(&mut self, width: usize, cell: Cell) {
        self.confirm_discard = false;
        self.buffer.place_cursor(width, cell);
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
        if let Some((stub, _, _)) = self.stub_on_row(view.cursor().row) {
            return vec![stub.id().clone()];
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

    // ----- comment box -----

    /// `c`: open the thread on the cursor row when there is one and
    /// nothing is selected (ADR 0027); otherwise the comment box on the
    /// selection, or the cursor line.
    pub(crate) fn start_comment(&mut self) {
        if self.view().selected_lines().is_some() {
            self.start_new_comment();
            return;
        }
        let row = self.view().cursor().row;
        // On an expanded thread's rows, `c` folds it and moves the cycle
        // on through the threads covering its last line (ADR 0049).
        if let Some((stub, _, _)) = self.stub_on_row(row) {
            let line = self
                .marks()
                .iter()
                .find(|mark| mark.id() == stub.id())
                .map_or(0, |mark| mark.range().end());
            let covering = self.threads_on_line(line);
            self.cycle_expanded(covering);
            return;
        }
        let covering = self.threads_at_cursor();
        if covering.is_empty() {
            self.start_new_comment();
            return;
        }
        self.cycle_expanded(covering);
    }

    /// The placed threads covering source line `line`, in line order.
    fn threads_on_line(&self, line: usize) -> Vec<ThreadId> {
        let lines = LineRange::new(line, line);
        self.file_threads()
            .into_iter()
            .filter(|id| {
                self.placed_marks()
                    .any(|mark| mark.id() == id && overlaps(mark.range(), lines))
            })
            .collect()
    }

    /// `C`: open the comment box on the selection, or the cursor line,
    /// whether or not a thread is already there.
    pub(crate) fn start_new_comment(&mut self) {
        let Some(doc) = self.current.and_then(|index| self.docs.get(index)) else {
            self.notice("open a file to annotate it");
            return;
        };
        // Threads anchor to lines, and these files have none (ADR 0026).
        match doc.document.content() {
            Content::Text(_) => {}
            Content::Binary { .. } => {
                self.notice("cannot annotate a binary file");
                return;
            }
            Content::TooLarge { .. } => {
                self.notice("cannot annotate a file this large");
                return;
            }
        }
        if self.store.is_none() {
            self.store_mut();
            return;
        }
        let view = self.view();
        // A detached thread's row is not text (ADR 0039).
        let on_detached_row = view.selected_lines().is_none()
            && view.detached_anchor_of_row(view.cursor().row).is_some();
        let range = view.selected_lines().or_else(|| {
            view.cursor_source_line()
                .map(|line| LineRange::new(line, line))
        });
        let Some(range) = range.filter(|_| !on_detached_row) else {
            self.notice("no lines here to annotate");
            return;
        };
        self.open_compose(ComposeTarget::New(range));
    }

    pub(super) fn open_compose(&mut self, target: ComposeTarget) {
        // A new thread would anchor to a snapshot no file matches, and a
        // reply would land on one (ADR 0028).
        let deleted = match &target {
            ComposeTarget::New(_) => self
                .current
                .and_then(|index| self.docs.get(index))
                .filter(|doc| doc.deleted.is_some())
                .map(|doc| doc.relative.clone()),
            ComposeTarget::Reply(id) | ComposeTarget::Edit { thread: id, .. } => self
                .thread(id)
                .map(|thread| thread.path().to_path_buf())
                .filter(|path| {
                    self.docs
                        .iter()
                        .any(|doc| doc.relative == *path && doc.deleted.is_some())
                }),
        };
        if let Some(path) = deleted {
            self.notice(format!("{} was deleted; cannot comment", path.display()));
            return;
        }
        let original = match &target {
            ComposeTarget::Edit { thread, message } => {
                let Some((body, editable)) = self.message_for(thread, *message) else {
                    self.notice("message no longer exists");
                    return;
                };
                if !editable {
                    self.notice("only your messages can be edited");
                    return;
                }
                body.to_owned()
            }
            ComposeTarget::New(_) | ComposeTarget::Reply(_) => String::new(),
        };
        let buffer = Buffer::from_text(&original);
        self.popup = Some(Popup::Compose(Compose {
            target,
            buffer,
            original,
            confirm_discard: false,
        }));
    }

    /// The open comment box, its discard prompt cleared: any edit means
    /// the draft is being kept.
    fn compose_mut(&mut self) -> Option<&mut Compose> {
        match self.popup.as_mut() {
            Some(Popup::Compose(compose)) => {
                compose.confirm_discard = false;
                Some(compose)
            }
            _ => None,
        }
    }

    /// A typed character or a paste, inserted at the cursor.
    pub(crate) fn compose_insert(&mut self, text: &str) {
        if let Some(compose) = self.compose_mut() {
            compose.buffer.insert(text);
        }
    }

    /// A motion or deletion in the comment (ADR 0018).
    pub(crate) fn compose_edit(&mut self, edit: Edit) {
        if let Some(compose) = self.compose_mut() {
            compose.buffer.apply(edit);
        }
    }

    /// The draft in the comment box, for the `$EDITOR` hatch.
    pub(crate) fn compose_draft(&self) -> Option<&str> {
        match &self.popup {
            Some(Popup::Compose(compose)) => Some(compose.buffer.text()),
            _ => None,
        }
    }

    /// Replace the whole draft (back from `$EDITOR`), cursor at the end.
    pub(crate) fn set_compose_text(&mut self, text: &str) {
        if let Some(compose) = self.compose_mut() {
            compose.buffer = Buffer::from_text(text);
        }
    }

    /// Alt-Up / Alt-Down while replying: scroll the text behind the box,
    /// where the thread is expanded (ADR 0049).
    pub(crate) fn compose_scroll(&mut self, delta: isize) {
        if matches!(self.popup, Some(Popup::Compose(_))) {
            self.view_mut().scroll_by(delta);
        }
    }

    /// Esc: drop the comment box; a reply returns to its thread's rows. A
    /// changed draft or edit asks for a second Esc first.
    pub(crate) fn compose_cancel(&mut self) {
        let Some(Popup::Compose(compose)) = self.popup.as_mut() else {
            return;
        };
        let editing = matches!(compose.target, ComposeTarget::Edit { .. });
        let changed = if editing {
            compose.buffer.text() != compose.original
        } else {
            !compose.buffer.text().trim().is_empty()
        };
        if !compose.confirm_discard && changed {
            compose.confirm_discard = true;
            self.notice(if editing {
                "Esc again to discard the edit"
            } else {
                "Esc again to discard the comment"
            });
            return;
        }
        self.popup = None;
        self.refocus_after_compose();
        self.notice(if editing {
            "edit cancelled"
        } else {
            "comment cancelled"
        });
    }

    /// Ctrl-C: wipe a non-empty draft in place; an empty box closes.
    pub(crate) fn compose_clear(&mut self) {
        let Some(Popup::Compose(compose)) = self.popup.as_mut() else {
            return;
        };
        let editing = matches!(compose.target, ComposeTarget::Edit { .. });
        if compose.buffer.text().trim().is_empty() {
            self.popup = None;
            self.refocus_after_compose();
            self.notice(if editing {
                "edit cancelled"
            } else {
                "comment cancelled"
            });
            return;
        }
        compose.buffer = Buffer::new();
        compose.confirm_discard = false;
    }

    /// Enter: write the comment, reply, or edit to the store.
    pub(crate) fn compose_submit(&mut self) {
        let Some(Popup::Compose(compose)) = self.popup.as_ref() else {
            return;
        };
        let text = compose.buffer.text().trim().to_owned();
        if text.is_empty() {
            if matches!(compose.target, ComposeTarget::Edit { .. }) {
                self.notice("a message cannot be empty");
                return;
            }
            self.popup = None;
            self.notice("empty comment discarded");
            return;
        }
        let Some(Popup::Compose(compose)) = self.popup.take() else {
            return;
        };
        match compose.target {
            ComposeTarget::New(range) => self.submit_annotation(range, text),
            ComposeTarget::Reply(id) => self.submit_reply(&id, text),
            ComposeTarget::Edit { thread, message } => {
                self.submit_message_edit(&thread, message, text);
            }
        }
        self.refocus_after_compose();
    }

    /// The box closed: the keys go back to the pane or the list it was
    /// opened from.
    fn refocus_after_compose(&mut self) {
        // A new message changes what the stubs show (ADR 0049).
        self.place_stub_rows();
        if self.focus == Focus::ThreadsPane && self.threads_pane_height() > 0 {
            // A reply from the threads pane keeps its keys (ADR 0034).
        } else if self.review_list.is_open() {
            self.focus = Focus::Review;
        }
    }

    fn submit_annotation(&mut self, range: LineRange, comment: String) {
        let Some(index) = self.current else {
            return;
        };
        let path = self.docs[index].relative.clone();
        let text = self.docs[index]
            .document
            .text()
            .unwrap_or_default()
            .to_owned();
        // The thread belongs to the work it was written against (ADR 0024).
        let draft = Draft::new(&path, range, comment).at_commit(self.workspace.head_commit());
        let Some(store) = self.store_mut() else {
            return;
        };
        match store.annotate(draft, &text, now()) {
            Ok(id) => {
                tracing::info!(%id, path = %path.display(), %range, "thread started");
                self.refresh_reach();
                self.refresh_marks(index);
                // Commenting on lines means they were read (ADR 0020).
                self.mark_seen(index);
                self.view_mut().clear_selection();
                self.notice(format!("commented on L{range}"));
            }
            Err(error) => self.notice(format!("cannot save comment: {error}")),
        }
    }

    fn submit_reply(&mut self, id: &ThreadId, body: String) {
        let Some(store) = self.store_mut() else {
            return;
        };
        match store.reply(id, Reply::new(Author::User, now(), body)) {
            Ok(()) => {
                tracing::info!(%id, "reply added");
                self.refresh_all_marks();
                // The reply becomes the highlighted message. The list and
                // the threads pane reply in place; a reply from the text
                // opens the thread it answered.
                if self.review_list.is_open() {
                    let newest = self.newest_message(id);
                    self.set_thread_cursor_message(id.clone(), newest);
                    self.review_follow_cursor();
                } else if self.focus != Focus::ThreadsPane {
                    // From the text the thread expands in place and the
                    // cursor lands on the reply (ADR 0049).
                    let newest = self.newest_message(id);
                    self.goto_message(id.clone(), newest);
                }
            }
            Err(error) => self.notice(format!("cannot save reply: {error}")),
        }
    }

    fn submit_message_edit(&mut self, id: &ThreadId, target: MessageTarget, body: String) {
        let Some(store) = self.store_mut() else {
            return;
        };
        match store.edit(id, target, body, now()) {
            Ok(()) => {
                tracing::info!(%id, ?target, "thread message edited");
                self.refresh_all_marks();
                self.follow_cursor_message();
                self.notice("message edited");
            }
            Err(error) => self.notice(format!("cannot edit message: {error}")),
        }
    }

    /// A reply arriving over the socket (ADR 0014), optionally resolving the
    /// thread; the open panel is refreshed when it shows that thread.
    pub(super) fn agent_reply(
        &mut self,
        id: &ThreadId,
        author: Author,
        body: String,
        resolve: bool,
        lines: Option<LineRange>,
    ) -> Result<(), String> {
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
        let reply = Reply::new(author.clone(), when, body);
        let reply = if resolve {
            reply.proposing_resolution()
        } else {
            reply
        };
        store.reply(id, reply).map_err(|e| e.to_string())?;
        if resolve {
            store.resolve(id, author, when).map_err(|e| e.to_string())?;
        }
        tracing::info!(%id, resolve, "agent reply added");
        // The toast a store reload would raise (ADR 0030), for the viewer
        // the reply came through; a resolving reply says so (ADR 0032).
        if let Some(thread) = store.thread(id) {
            let place = format!("{}:{}", thread.path().display(), thread.range().start());
            self.push_toast(if resolve {
                format!("reply on {place}, resolved")
            } else {
                format!("reply on {place}")
            });
        }
        for index in 0..self.docs.len() {
            self.refresh_marks(index);
        }
        Ok(())
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
            MessageTarget::Comment => Some((thread.comment(), true)),
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
            store.resolve(id, Author::User, now())
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
