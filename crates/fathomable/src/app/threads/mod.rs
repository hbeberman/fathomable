// @okf-doc: /decisions/0013-annotation-storage-and-ux.md
//! Threads on top of the view: the store, the marks, the comment box, and
//! the thread pane, with the thread concept's other modules beneath.
//!
//! [`App`] keeps the [`Store`] for the workspace; every open document
//! carries the [`Mark`]s of its threads, re-located whenever the text
//! changes. The comment box ([`Compose`]) starts a thread or replies to
//! one; the [`ThreadPane`] reads a thread and resolves it. The submodules
//! are the thread cursor (`cursor`), the file-threads pane (`file_pane`),
//! the thread list (`list`), deletion, detached rows, the open thread's
//! lines (`open`), git reach, re-anchoring, waiting threads, and the
//! placement and state words. All of it is plain state, tested without a
//! terminal (ADRs 0013 and 0046).

pub(crate) mod cursor;
pub(crate) mod delete;
pub(crate) mod detached;
pub(crate) mod file_pane;
pub(crate) mod list;
pub(crate) mod open;
pub(crate) mod reach;
pub(crate) mod reanchor;
pub(crate) mod waiting;
pub(crate) mod words;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fathomable_core::annotations::{
    Author, Draft, LineRange, MessageTarget, Party, Placement, Reply, Status, Store, Thread,
    ThreadId,
};
use fathomable_core::content::Content;
use fathomable_core::editor::{Buffer, Cell, Edit};
use fathomable_core::reanchor::{Mapping, map_range};

use crate::app::{App, Focus, Popup};

/// A thread's status, which is its colour in the gutter, the file-threads
/// pane, and the thread list (ADR 0039); where the thread is placed is
/// [`Mark::placement`]. Ordered by urgency, so the most urgent of several
/// on one row is their `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThreadState {
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
pub struct Mark {
    id: ThreadId,
    placement: Placement,
    kind: ThreadState,
}

impl Mark {
    pub fn id(&self) -> &ThreadId {
        &self.id
    }

    pub fn range(&self) -> LineRange {
        self.placement.range()
    }

    pub fn kind(&self) -> ThreadState {
        self.kind
    }

    /// Where the thread sits in the current text.
    pub fn placement(&self) -> Placement {
        self.placement
    }

    /// Whether the annotated lines are gone: the thread is shown on a
    /// row of its own (ADR 0039), not at its last known range.
    pub fn is_detached(&self) -> bool {
        self.placement.is_detached()
    }
}

/// What the comment box will produce on submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeTarget {
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
pub struct Compose {
    target: ComposeTarget,
    buffer: Buffer,
    /// The text the box opened with, empty for a new comment or reply.
    original: String,
    /// Esc was pressed on a non-empty draft; the next Esc discards it.
    confirm_discard: bool,
}

impl Compose {
    pub fn target(&self) -> &ComposeTarget {
        &self.target
    }

    /// The draft and its cursor (ADR 0018).
    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Whether the box is asking for a second Esc.
    pub fn confirming_discard(&self) -> bool {
        self.confirm_discard
    }

    /// A mouse click at a wrapped cell moves the cursor there.
    pub fn place_cursor(&mut self, width: usize, cell: Cell) {
        self.confirm_discard = false;
        self.buffer.place_cursor(width, cell);
    }
}

/// The open thread pane: how far the cursor's thread is scrolled. Which
/// thread it shows is the thread cursor's (ADR 0046); its place among the
/// file's threads is computed from the document's marks
/// ([`App::thread_position`]), so a reload cannot strand it (ADR 0027).
#[derive(Debug)]
pub struct ThreadPane {
    scroll: usize,
    /// Number of messages when the scroll was last set.
    seen_messages: usize,
    /// The thread's `updated` when the scroll was last set, so a new
    /// reply sends the pane back to its end (ADR 0034).
    seen: u64,
}

/// A scroll past any body: the drawing and `thread_scroll` clamp it to
/// the last row, so the pane opens at its end (ADR 0034).
const BOTTOM: usize = usize::MAX;

impl ThreadPane {
    pub fn scroll(&self) -> usize {
        self.scroll
    }
}

pub(super) fn overlaps(a: LineRange, b: LineRange) -> bool {
    a.start() <= b.end() && b.start() <= a.end()
}

/// Seconds since the Unix epoch.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
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
    pub fn thread(&self, id: &ThreadId) -> Option<&Thread> {
        self.store.as_ref()?.thread(id)
    }

    /// Marks of the current document, oldest thread first.
    pub fn marks(&self) -> &[Mark] {
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
    pub fn mark_in(&self, lines: LineRange) -> Option<ThreadState> {
        self.placed_marks()
            .filter(|mark| overlaps(mark.range(), lines))
            .map(Mark::kind)
            .max()
    }

    /// `(open, total)` threads on the current document.
    pub fn thread_counts(&self) -> (usize, usize) {
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
        // The file-threads pane's height follows the marks (ADR 0027), and
        // the detached ones take rows of their own (ADR 0039).
        if self.current == Some(index) {
            self.place_detached_rows();
            self.scroll_sidebar();
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
    /// order the store holds them (ADR 0027). This is the order `l` / `h`
    /// in the thread pane and `j` / `k` in the file-threads pane walk.
    pub fn file_threads(&self) -> Vec<ThreadId> {
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

    /// `(current, total)`, 1-based, of the cursor's thread among the
    /// file's, for the pane header.
    pub fn thread_position(&self) -> Option<(usize, usize)> {
        let cursor = self.thread_cursor();
        let id = cursor.thread()?;
        let order = self.file_threads();
        let index = order.iter().position(|other| other == id)?;
        Some((index + 1, order.len()))
    }

    /// `(current, total)`, 1-based, of the cursor's thread among the
    /// workspace's, for the pane header.
    pub fn thread_position_across(&self) -> Option<(usize, usize)> {
        let cursor = self.thread_cursor();
        let id = cursor.thread()?;
        let order = self.workspace_threads();
        let index = order.iter().position(|other| other == id)?;
        Some((index + 1, order.len()))
    }

    /// Threads whose range touches the cursor's rendered row, or the
    /// detached threads standing on it (ADR 0039).
    pub fn threads_at_cursor(&self) -> Vec<ThreadId> {
        let view = self.view();
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
    pub fn start_comment(&mut self) {
        if self.view().selected_lines().is_none() && !self.threads_at_cursor().is_empty() {
            self.open_thread_at_cursor();
            return;
        }
        self.start_new_comment();
    }

    /// `C`: open the comment box on the selection, or the cursor line,
    /// whether or not a thread is already there.
    pub fn start_new_comment(&mut self) {
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
    pub fn compose_insert(&mut self, text: &str) {
        if let Some(compose) = self.compose_mut() {
            compose.buffer.insert(text);
        }
    }

    /// A motion or deletion in the comment (ADR 0018).
    pub fn compose_edit(&mut self, edit: Edit) {
        if let Some(compose) = self.compose_mut() {
            compose.buffer.apply(edit);
        }
    }

    /// The draft in the comment box, for the `$EDITOR` hatch.
    pub fn compose_draft(&self) -> Option<&str> {
        match &self.popup {
            Some(Popup::Compose(compose)) => Some(compose.buffer.text()),
            _ => None,
        }
    }

    /// Replace the whole draft (back from `$EDITOR`), cursor at the end.
    pub fn set_compose_text(&mut self, text: &str) {
        if let Some(compose) = self.compose_mut() {
            compose.buffer = Buffer::from_text(text);
        }
    }

    /// `PageUp` / `PageDown` / Alt-Up / Alt-Down while replying: scroll the
    /// thread shown above the box.
    pub fn compose_scroll(&mut self, delta: isize) {
        if matches!(self.popup, Some(Popup::Compose(_))) {
            self.thread_scroll(delta);
        }
    }

    /// Esc: drop the comment box; a reply returns to its thread pane. A
    /// changed draft or edit asks for a second Esc first.
    pub fn compose_cancel(&mut self) {
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
    pub fn compose_clear(&mut self) {
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
    pub fn compose_submit(&mut self) {
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
        if self.focus == Focus::FileThreads && self.file_thread_pane_rows() > 0 {
            // A reply from the file-threads pane keeps its keys (ADR 0034).
        } else if self.thread.is_some() {
            self.focus = Focus::Thread;
        } else if self.list.is_open() {
            self.focus = Focus::Threads;
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
                // the file-threads pane reply in place; a reply from the
                // text opens the thread it answered.
                if self.list.is_open() {
                    let newest = self.newest_message(id);
                    self.set_thread_cursor_message(id.clone(), newest);
                    self.thread_list_follow_cursor();
                } else if self.focus != Focus::FileThreads {
                    self.open_thread(id.clone());
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
                if let Some(updated) = self.thread(id).map(Thread::updated)
                    && self.thread_cursor().thread() == Some(id)
                    && let Some(panel) = self.thread.as_mut()
                {
                    panel.seen = updated;
                }
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
        if self.pane_thread() == Some(id) {
            self.open_thread(id.clone());
        }
        Ok(())
    }

    // ----- thread pane -----

    /// `Space a`: the pane on the first thread under the cursor, the
    /// others one `l` away; on the focused pane, close it instead.
    pub fn toggle_thread_pane(&mut self) {
        if self.thread.is_some() && self.focus == Focus::Thread {
            self.close_thread();
        } else {
            self.open_thread_at_cursor();
        }
    }

    /// Show the first thread under the cursor; the others on the row are
    /// one `l` away.
    pub fn open_thread_at_cursor(&mut self) {
        let Some(id) = self.threads_at_cursor().into_iter().next() else {
            self.notice("no thread on this line");
            return;
        };
        self.open_thread(id);
    }

    /// Show one thread at its end, keeping the scroll and the highlighted
    /// message only when the pane already shows it and nothing was added
    /// (ADR 0034). The cursor moves onto the thread (ADR 0046).
    pub fn open_thread(&mut self, id: ThreadId) {
        self.refresh_watchers();
        let updated = self.thread(&id).map_or(0, Thread::updated);
        let messages = self
            .thread(&id)
            .map_or(0, |thread| thread.replies().len() + 1);
        let kept = self
            .thread
            .as_ref()
            .filter(|panel| {
                self.thread_cursor.thread() == Some(&id)
                    && panel.seen == updated
                    && panel.seen_messages == messages
            })
            .map(|panel| panel.scroll);
        let message = match kept {
            Some(_) => self.thread_cursor.message(),
            None => self.newest_message(&id),
        };
        self.set_thread_cursor_message(id, message);
        self.show_panel(ThreadPane {
            scroll: kept.unwrap_or(BOTTOM),
            seen_messages: messages,
            seen: updated,
        });
        // Resolve `BOTTOM` to the real last row now the pane has a height.
        self.thread_scroll(0);
    }

    /// Show `id` without taking the keys from the pane that asked: the
    /// file-threads pane drives the thread pane (ADR 0034).
    pub(super) fn open_thread_behind(&mut self, id: ThreadId) {
        let focus = self.focus;
        self.open_thread(id);
        self.focus = focus;
    }

    /// The pane opens along the bottom of the text and takes the keys
    /// unless the comment box is up.
    fn show_panel(&mut self, panel: ThreadPane) {
        self.thread = Some(panel);
        if self.popup.is_none() {
            self.focus = Focus::Thread;
        }
        self.relayout();
    }

    /// Esc in the pane: the keys go back to the text and the pane stays
    /// (ADR 0010, amended 2026-09-03); `Space a` closes it.
    pub fn leave_thread_pane(&mut self) {
        if self.focus == Focus::Thread {
            self.focus = Focus::View;
        }
    }

    /// `Space a` on the focused pane, the list taking the column, or a
    /// deleted thread: close the pane and hand the keys back to the text.
    pub fn close_thread(&mut self) {
        self.thread = None;
        if self.focus == Focus::Thread {
            self.focus = Focus::View;
        }
        self.relayout();
    }

    fn panel_mut(&mut self) -> Option<&mut ThreadPane> {
        self.thread.as_mut()
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

    /// Scroll the panel text, stopping at the last row.
    pub fn thread_scroll(&mut self, delta: isize) {
        let Some(id) = self.thread_cursor().thread().cloned() else {
            return;
        };
        let Some(panel) = self.panel_mut() else {
            return;
        };
        let scroll = panel.scroll;
        // The limit is the body as drawn, wrapped at the column's width,
        // so a half page from the end moves at once (ADR 0034).
        let body_rows = self.thread_rows().saturating_sub(2);
        let width = self.width.saturating_sub(self.sidebar_width()).max(1);
        let limit = self
            .thread(&id)
            .map_or(0, |thread| {
                crate::app::draw::message::thread_body_rows(thread, width, &self.highlighter)
            })
            .saturating_sub(body_rows);
        if let Some(panel) = self.panel_mut() {
            panel.scroll = scroll.min(limit).saturating_add_signed(delta).min(limit);
        }
    }

    /// `Ctrl-d` / `Ctrl-u` in the pane: scroll by half its body rows.
    pub fn thread_scroll_half_page(&mut self, direction: isize) {
        let half = isize::try_from(self.thread_rows().saturating_sub(2) / 2)
            .unwrap_or(isize::MAX)
            .max(1);
        self.thread_scroll(direction.signum() * half);
    }

    pub(super) fn thread_message_into_view(&mut self) {
        let cursor = self.thread_cursor();
        let Some(id) = cursor.thread().cloned() else {
            return;
        };
        let Some(panel) = self.thread.as_ref() else {
            return;
        };
        let selected = cursor.message();
        let scroll = panel.scroll;
        let body_rows = self.thread_rows().saturating_sub(2).max(1);
        let width = self.width.saturating_sub(self.sidebar_width()).max(1);
        let Some(thread) = self.thread(&id) else {
            return;
        };
        let total = crate::app::draw::message::thread_body_rows(thread, width, &self.highlighter);
        let Some(range) = crate::app::draw::message::thread_message_range(
            thread,
            selected,
            width,
            &self.highlighter,
        ) else {
            return;
        };
        let limit = total.saturating_sub(body_rows);
        let next = if range.start < scroll {
            range.start
        } else if range.end > scroll.saturating_add(body_rows) {
            if range.len() > body_rows {
                range.start
            } else {
                range.end.saturating_sub(body_rows)
            }
        } else {
            scroll
        };
        if let Some(panel) = self.panel_mut() {
            panel.scroll = next.min(limit);
        }
    }

    /// `h` on the file's first thread: the keys go to the file-threads
    /// pane, the tree shown first if it was hidden (ADR 0034).
    pub fn thread_to_file_threads(&mut self) {
        if self.marks().is_empty() {
            return;
        }
        if self.tree().is_none() {
            self.show_sidebar();
        }
        if self.file_thread_pane_rows() > 0 {
            self.focus = Focus::FileThreads;
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
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::annotations::{LineRange, MessageTarget, Status, Store, Thread};
    use fathomable_core::workspace::Workspace;

    use fathomable_core::annotations::Author;
    use fathomable_core::session::{Request, Response};

    use anyhow::Context as _;

    use crate::app::Focus;
    use fathomable_testing::TempDir;

    use fathomable_core::editor::{Cursor, Edit, Motion};

    use super::{ComposeTarget, ThreadState};
    use crate::app::threads::list::Row;
    use crate::app::{App, Border, Options, Popup};

    fn fixture(name: &str) -> std::io::Result<TempDir> {
        let dir = TempDir::new(&format!("threads-{name}"))?;
        fs::create_dir_all(dir.0.join("ws"))?;
        fs::write(
            dir.0.join("ws/README.md"),
            "# Readme\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
        )?;
        Ok(dir)
    }

    fn app(dir: &TempDir) -> anyhow::Result<App> {
        let workspace = Workspace::discover(dir.0.join("ws"))?;
        let store = Store::open(dir.0.join("state/threads.jsonl"))?;
        let options = Options {
            store: Some(store),
            ..Options::for_test(dir.0.join("ws"))
        };
        let mut app = App::new(workspace, 100, 30, options);
        app.open(Path::new("README.md"));
        Ok(app)
    }

    fn type_in(app: &mut App, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                app.compose_edit(Edit::Newline);
            } else {
                app.compose_insert(&ch.to_string());
            }
        }
    }

    /// Annotate L3-5 of the open README with `comment`.
    fn annotate(app: &mut App, comment: &str) -> anyhow::Result<()> {
        app.view_mut().move_down(2);
        app.view_mut().select_lines();
        app.start_comment();
        type_in(app, comment);
        app.compose_submit();
        anyhow::ensure!(app.thread_counts().1 == 1, "thread not created");
        Ok(())
    }

    fn app_with_thread_list_messages(
        name: &str,
    ) -> anyhow::Result<(TempDir, App, fathomable_core::annotations::ThreadId)> {
        let dir = fixture(name)?;
        let mut app = app(&dir)?;
        let opening = (1..=30)
            .map(|line| format!("opening line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        annotate(&mut app, &opening)?;
        let id = app.marks()[0].id().clone();
        app.agent_reply(
            &id,
            Author::agent("reviewer"),
            "agent answer".to_owned(),
            false,
            None,
        )
        .map_err(anyhow::Error::msg)?;
        app.open_thread(id.clone());
        app.thread_reply();
        type_in(&mut app, "user follow-up");
        app.compose_submit();
        app.close_thread();
        app.view_mut().goto_bottom();
        app.start_comment();
        type_in(&mut app, "bottom");
        app.compose_submit();
        app.view_mut().goto_top();
        app.open_thread_list();
        Ok((dir, app, id))
    }

    #[test]
    fn a_rename_carries_the_threads_and_the_open_document() -> anyhow::Result<()> {
        use crate::app::watch::Event;
        let dir = fixture("rename")?;
        let mut app = app(&dir)?;
        annotate(&mut app, "keep me")?;
        let id = app.marks()[0].id().clone();
        app.view_mut().move_down(1);
        let cursor = app.view().cursor();

        // A file rename: the view follows with cursor and marks intact
        // and the store records the move.
        fs::create_dir_all(dir.0.join("ws/docs"))?;
        fs::rename(dir.0.join("ws/README.md"), dir.0.join("ws/docs/GUIDE.md"))?;
        app.on_events(vec![Event::Renamed {
            from: dir.0.join("ws/README.md"),
            to: dir.0.join("ws/docs/GUIDE.md"),
        }]);
        assert_eq!(app.current_path(), Path::new("docs/GUIDE.md"));
        assert_eq!(app.message(), Some("renamed to docs/GUIDE.md"));
        assert_eq!(app.view().cursor(), cursor);
        assert_eq!(app.marks()[0].range(), LineRange::new(3, 5));
        assert_eq!(app.mark_in(LineRange::new(4, 4)), Some(ThreadState::Open));
        assert_eq!(
            app.thread(&id).map(Thread::path),
            Some(Path::new("docs/GUIDE.md"))
        );
        let store = Store::open(dir.0.join("state/threads.jsonl"))?;
        assert_eq!(
            store.thread(&id).map(Thread::path),
            Some(Path::new("docs/GUIDE.md")),
            "the move is on disk"
        );

        // A directory rename moves everything under it by prefix.
        fs::rename(dir.0.join("ws/docs"), dir.0.join("ws/notes"))?;
        app.on_events(vec![Event::Renamed {
            from: dir.0.join("ws/docs"),
            to: dir.0.join("ws/notes"),
        }]);
        assert_eq!(app.current_path(), Path::new("notes/GUIDE.md"));
        assert_eq!(
            app.thread(&id).map(Thread::path),
            Some(Path::new("notes/GUIDE.md"))
        );
        assert_eq!(app.thread_counts(), (1, 1));

        // A later edit reloads from the new path.
        fs::write(
            dir.0.join("ws/notes/GUIDE.md"),
            "# Readme\n\nintro\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
        )?;
        app.on_events(vec![Event::Change(dir.0.join("ws/notes/GUIDE.md"))]);
        assert!(app.view().text().contains("intro"));
        assert_eq!(app.marks()[0].range(), LineRange::new(5, 7));
        Ok(())
    }

    #[test]
    fn a_deleted_file_keeps_its_content_and_refuses_new_comments() -> anyhow::Result<()> {
        use crate::app::watch::Event;
        let dir = fixture("deleted")?;
        let mut app = app(&dir)?;
        annotate(&mut app, "still here")?;
        let id = app.marks()[0].id().clone();
        fs::write(dir.0.join("ws/other.md"), "# Other\n")?;

        fs::remove_file(dir.0.join("ws/README.md"))?;
        app.on_events(vec![Event::Removed(dir.0.join("ws/README.md"))]);
        assert!(app.deleted());
        assert_eq!(app.banner(), Some("deleted"));
        assert!(app.view().text().contains("alpha"), "last content stays");
        assert_eq!(app.thread_counts(), (1, 1), "threads still read");
        assert!(
            app.status_lines()
                .iter()
                .any(|(_, v)| v.contains("deleted"))
        );
        app.start_new_comment();
        assert!(app.popup().is_none());
        assert!(app.message().is_some_and(|m| m.contains("deleted")));
        app.open_thread(id.clone());
        app.thread_reply();
        assert!(!matches!(app.popup(), Some(Popup::Compose(_))));
        app.close_thread();

        // Shown again while still gone: the file-info pane.
        app.open(Path::new("other.md"));
        assert!(app.info().is_none());
        app.open(Path::new("README.md"));
        let info = app.info().context("no info pane for the deleted file")?;
        assert!(info.rows.iter().any(|(_, v)| v == "deleted"));
        assert_eq!(app.banner(), None);

        // Back on disk: reloaded, banner gone, thread re-anchored.
        fs::write(
            dir.0.join("ws/README.md"),
            "# Readme\n\nintro\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
        )?;
        app.on_events(vec![Event::Created(dir.0.join("ws/README.md"))]);
        assert!(!app.deleted());
        assert!(app.info().is_none());
        assert!(app.view().text().contains("intro"));
        assert_eq!(app.marks()[0].range(), LineRange::new(5, 7));
        app.start_new_comment();
        assert!(matches!(app.popup(), Some(Popup::Compose(_))));
        Ok(())
    }

    #[test]
    fn selection_becomes_a_thread_and_survives_reload() -> anyhow::Result<()> {
        let dir = fixture("annotate")?;
        let mut app = app(&dir)?;
        assert_eq!(app.thread_counts(), (0, 0));
        // Rows: 0 "# Readme", 1 blank, 2 "alpha beta gamma" (one paragraph).
        app.view_mut().move_down(2);
        app.view_mut().select_lines();
        app.start_comment();
        let Some(Popup::Compose(compose)) = app.popup() else {
            anyhow::bail!("comment box did not open");
        };
        assert_eq!(compose.target(), &ComposeTarget::New(LineRange::new(3, 5)));
        type_in(&mut app, "tighten\nthis");
        app.compose_submit();
        assert!(app.popup().is_none());
        assert_eq!(app.message(), Some("commented on L3-5"));
        assert_eq!(app.thread_counts(), (1, 1));
        assert_eq!(app.mark_in(LineRange::new(4, 4)), Some(ThreadState::Open));
        assert_eq!(app.mark_in(LineRange::new(1, 1)), None);
        assert!(
            app.view().selection().is_none(),
            "selection cleared after commenting"
        );

        // Insert lines above: the mark follows the content.
        fs::write(
            dir.0.join("ws/README.md"),
            "# Readme\n\nnew intro\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
        )?;
        app.on_changes(vec![dir.0.join("ws/README.md")]);
        assert_eq!(app.marks()[0].range(), LineRange::new(5, 7));
        assert_eq!(app.mark_in(LineRange::new(3, 3)), None);

        // Edit one of them: the thread follows onto the rewritten lines
        // and reads as edited, on disk too (ADR 0019).
        fs::write(
            dir.0.join("ws/README.md"),
            "# Readme\n\nnew intro\n\nalpha\nBETA\ngamma\n\n- one\n- two\n",
        )?;
        app.on_changes(vec![dir.0.join("ws/README.md")]);
        assert_eq!(app.marks()[0].range(), LineRange::new(5, 7));
        assert!(app.marks()[0].placement().is_edited());
        assert_eq!(app.mark_in(LineRange::new(6, 6)), Some(ThreadState::Open));
        let reopened = Store::open(dir.0.join("state/threads.jsonl"))?;
        assert!(reopened.threads()[0].edited().is_some());
        assert_eq!(reopened.threads()[0].range(), LineRange::new(5, 7));

        // The user's reply acknowledges the edit.
        app.view_mut().move_down(3);
        app.open_thread_at_cursor();
        app.thread_reply();
        type_in(&mut app, "still fine");
        app.compose_submit();
        assert_eq!(app.mark_in(LineRange::new(6, 6)), Some(ThreadState::Open));

        // Rewrite everything: the thread detaches at its last known range.
        fs::write(dir.0.join("ws/README.md"), "# Readme\n\ngone\n")?;
        app.on_changes(vec![dir.0.join("ws/README.md")]);
        assert!(app.marks()[0].is_detached());
        assert_eq!(app.mark_in(LineRange::new(5, 5)), None);
        // Placement and state are told apart (ADR 0032).
        let rows = app.file_thread_rows();
        assert_eq!(rows[0].words().placement(), Some("detached"));
        assert_eq!(rows[0].words().state(), ThreadState::Open);
        Ok(())
    }

    #[test]
    fn thread_panel_renders_header_snippet_badge_and_overflow() -> anyhow::Result<()> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let dir = fixture("render")?;
        let mut app = app(&dir)?;
        app.resize(80, 24);
        app.view_mut().move_down(2);
        app.view_mut().select_lines();
        app.start_comment();
        type_in(&mut app, "What is this?");
        app.compose_submit();
        let id = app.marks()[0].id().clone();
        let long = (1..=12)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.agent_reply(
            &id,
            Author::Agent {
                name: "Copilot".to_owned(),
                client: Some("github-copilot-developer".to_owned()),
                id: None,
                kind: None,
            },
            long,
            true,
            None,
        )
        .map_err(anyhow::Error::msg)?;
        app.open_thread(id);

        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::draw::Theme::from_core(&core);
        let render = |app: &App| -> anyhow::Result<Vec<String>> {
            let mut terminal = Terminal::new(TestBackend::new(80, 36))?;
            terminal.draw(|frame| crate::app::draw::draw(frame, app, &theme))?;
            let buffer = terminal.backend().buffer().clone();
            Ok((0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol().to_owned())
                        .collect::<String>()
                })
                .skip_while(|row| !row.contains(" thread "))
                .collect())
        };
        app.resize(80, 36);
        // The pane opens at its end, under the END row (ADR 0034).
        let panel = render(&app)?;
        let screen = panel.join("\n");
        assert!(
            panel.iter().any(|row| row.contains("─── END ───")),
            "end marker:\n{screen}"
        );
        assert!(
            !panel.iter().any(|row| row.contains("▼")),
            "nothing below at the end:\n{screen}"
        );
        app.thread_scroll(-100);
        assert_eq!(app.thread_panel().map(super::ThreadPane::scroll), Some(0));
        let panel = render(&app)?;
        let screen = panel.join("\n");
        assert!(
            panel[0].contains("thread 1/1 in file · 1/1 overall · L3-5 · auto-resolved"),
            "header carries range and status:\n{screen}"
        );
        assert!(
            !panel.iter().any(|row| row.contains("─── END ───")),
            "the end is off screen at the top:\n{screen}"
        );
        assert!(
            panel.iter().any(|row| row.contains("3 │ alpha")),
            "snippet lines are numbered:\n{screen}"
        );
        assert!(
            panel
                .iter()
                .any(|row| row.contains("Copilot") && row.contains("[proposes resolving]")),
            "short author and badge share the row:\n{screen}"
        );
        assert!(
            !screen.contains("github-copilot-developer"),
            "client id is not shown"
        );
        assert!(
            panel
                .iter()
                .any(|row| row.contains("▼") && row.contains("more")),
            "overflow indicator:\n{screen}"
        );
        for _ in 0..100 {
            app.thread_scroll(1);
        }
        let Some(panel) = app.thread_panel() else {
            anyhow::bail!("panel closed");
        };
        assert!(panel.scroll() < 40, "scroll clamps to the thread length");
        Ok(())
    }

    #[test]
    fn thread_panel_replies_resolves_and_reopens() -> anyhow::Result<()> {
        let dir = fixture("panel")?;
        let mut app = app(&dir)?;
        app.open_thread_at_cursor();
        assert_eq!(app.message(), Some("no thread on this line"));
        app.start_comment();
        type_in(
            &mut app,
            &(1..=20)
                .map(|n| format!("first {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        app.compose_submit();
        assert_eq!(app.mark_in(LineRange::new(1, 1)), Some(ThreadState::Open));

        app.open_thread_at_cursor();
        anyhow::ensure!(app.thread_panel().is_some(), "panel did not open");
        let id = app.thread_cursor().thread().cloned().context("no cursor")?;
        assert_eq!(app.thread_position(), Some((1, 1)));
        assert_eq!(app.focus(), Focus::Thread);
        app.thread_scroll(-100);
        app.thread_reply();
        assert!(
            matches!(app.popup(), Some(Popup::Compose(c)) if c.target() == &ComposeTarget::Reply(id.clone()))
        );
        assert!(
            app.thread_panel().is_some(),
            "the thread stays readable while replying"
        );
        app.compose_scroll(2);
        app.compose_cancel();
        assert!(app.popup().is_none());
        assert!(
            app.thread_panel().is_some_and(|p| p.scroll() == 2),
            "Esc returns to the pane"
        );
        assert_eq!(app.focus(), Focus::Thread);
        app.thread_reply();
        type_in(&mut app, "second thoughts");
        app.compose_submit();
        assert!(app.popup().is_none());
        assert!(
            app.thread_panel().is_some(),
            "pane stays open after a reply"
        );
        assert_eq!(app.focus(), Focus::Thread);
        let thread = app.thread(&id).cloned();
        assert_eq!(thread.as_ref().map(|t| t.replies().len()), Some(1));

        app.thread_toggle_resolved();
        assert_eq!(app.thread(&id).map(Thread::status), Some(Status::Resolved));
        assert_eq!(
            app.mark_in(LineRange::new(1, 1)),
            Some(ThreadState::Resolved)
        );
        assert_eq!(app.thread_counts(), (0, 1));
        app.thread_toggle_resolved();
        assert_eq!(app.thread(&id).map(Thread::status), Some(Status::Open));

        // Everything is on disk for the next session.
        let again = Store::open(dir.0.join("state/threads.jsonl"))?;
        assert_eq!(again.threads().len(), 1);
        assert_eq!(again.threads()[0].replies()[0].body(), "second thoughts");
        Ok(())
    }

    #[test]
    fn thread_keys_select_messages_and_edit_only_the_users() -> anyhow::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::app::input::keys;

        let dir = fixture("message-nav")?;
        let mut app = app(&dir)?;
        annotate(&mut app, "opening")?;
        let id = app.marks()[0].id().clone();
        app.agent_reply(
            &id,
            Author::agent("reviewer"),
            "agent answer".to_owned(),
            false,
            None,
        )
        .map_err(anyhow::Error::msg)?;
        app.open_thread(id.clone());
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        assert_eq!(
            app.thread_panel().map(|_| app.thread_cursor().message()),
            Some(1),
            "the newest message starts selected"
        );
        keys::handle_key(&mut app, key(KeyCode::Char('e')));
        assert!(app.popup().is_none());
        assert_eq!(app.message(), Some("only your messages can be edited"));

        keys::handle_key(&mut app, key(KeyCode::Char('k')));
        assert_eq!(
            app.thread_panel().map(|_| app.thread_cursor().message()),
            Some(0)
        );
        keys::handle_key(&mut app, key(KeyCode::Char('e')));
        assert!(matches!(
            app.popup(),
            Some(Popup::Compose(compose))
                if compose.target()
                    == &ComposeTarget::Edit {
                        thread: id.clone(),
                        message: MessageTarget::Comment,
                    }
        ));
        assert_eq!(app.compose_draft(), Some("opening"));
        app.set_compose_text("revised opening");
        app.compose_submit();
        assert_eq!(
            app.thread(&id).map(Thread::comment),
            Some("revised opening")
        );

        app.thread_reply();
        type_in(&mut app, "user follow-up");
        app.compose_submit();
        assert_eq!(
            app.thread_panel().map(|_| app.thread_cursor().message()),
            Some(2)
        );
        keys::handle_key(&mut app, key(KeyCode::Char('e')));
        assert!(matches!(
            app.popup(),
            Some(Popup::Compose(compose))
                if compose.target()
                    == &ComposeTarget::Edit {
                        thread: id.clone(),
                        message: MessageTarget::Reply(1),
                    }
        ));
        assert_eq!(app.compose_draft(), Some("user follow-up"));
        app.compose_cancel();
        assert!(app.popup().is_none(), "an unchanged edit closes at once");
        keys::handle_key(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.thread_cursor().message(), 1);
        keys::handle_key(&mut app, key(KeyCode::Char('g')));
        keys::handle_key(&mut app, key(KeyCode::Char('g')));
        assert_eq!(app.thread_cursor().message(), 0, "gg is the comment");
        keys::handle_key(&mut app, key(KeyCode::Char('G')));
        assert_eq!(app.thread_cursor().message(), 2, "G is the newest");
        Ok(())
    }

    #[test]
    fn thread_list_keys_select_messages_and_edit_only_the_users() -> anyhow::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::app::input::keys;

        let (_dir, mut app, id) = app_with_thread_list_messages("list-message-nav")?;
        let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

        assert_eq!(
            app.thread_cursor().message(),
            2,
            "the newest message starts selected"
        );
        let rows = app.thread_list_rows(60);
        let selected_row = rows
            .rows
            .iter()
            .position(|row| matches!(row, Row::Message { selected: true, .. }))
            .context("no selected message row")?;
        let visible = app.text_rows().saturating_sub(1).max(1);
        assert!(
            (app.thread_list().scroll()..app.thread_list().scroll() + visible)
                .contains(&selected_row),
            "the selected message is visible"
        );

        keys::handle_key(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.thread_cursor().message(), 1);
        keys::handle_key(&mut app, key(KeyCode::Char('e')));
        assert!(app.popup().is_none());
        assert_eq!(app.message(), Some("only your messages can be edited"));

        keys::handle_key(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.thread_cursor().message(), 0);
        keys::handle_key(&mut app, key(KeyCode::Char('e')));
        assert!(matches!(
            app.popup(),
            Some(Popup::Compose(compose))
                if compose.target()
                    == &ComposeTarget::Edit {
                        thread: id.clone(),
                        message: MessageTarget::Comment,
                    }
        ));
        app.set_compose_text("revised opening");
        app.compose_submit();
        assert_eq!(app.focus(), Focus::Threads);
        assert!(app.thread_list().is_open());
        assert_eq!(
            app.thread(&id).map(Thread::comment),
            Some("revised opening")
        );

        keys::handle_key(&mut app, key(KeyCode::Char('l')));
        assert_eq!(app.thread_cursor().message(), 0);
        keys::handle_key(&mut app, key(KeyCode::Char('h')));
        assert_eq!(
            app.thread_cursor().message(),
            2,
            "changing threads selects the newest message"
        );
        keys::handle_key(&mut app, key(KeyCode::Char('e')));
        assert!(matches!(
            app.popup(),
            Some(Popup::Compose(compose))
                if compose.target()
                    == &ComposeTarget::Edit {
                        thread: id.clone(),
                        message: MessageTarget::Reply(1),
                    }
        ));
        assert_eq!(app.compose_draft(), Some("user follow-up"));
        app.compose_cancel();
        assert_eq!(app.focus(), Focus::Threads);
        Ok(())
    }

    #[test]
    fn thread_list_mouse_selects_messages_and_reply_selects_itself() -> anyhow::Result<()> {
        let (_dir, mut app, id) = app_with_thread_list_messages("list-message-mouse")?;
        let rows = app.thread_list_rows(100);
        let agent_row = rows
            .rows
            .iter()
            .position(|row| {
                matches!(
                    row,
                    Row::Message {
                        entry: 0,
                        message: 1,
                        ..
                    }
                )
            })
            .context("no agent message row")?;
        let scroll = app.thread_list().scroll();
        anyhow::ensure!(agent_row >= scroll, "agent message is above the viewport");
        app.thread_list_click(agent_row - scroll);
        assert_eq!(
            app.thread_cursor().message(),
            1,
            "a click selects its message"
        );
        app.thread_reply();
        type_in(&mut app, "reply from the list");
        app.compose_submit();
        assert_eq!(
            app.thread_cursor().message(),
            3,
            "a reply sent from the list becomes selected"
        );
        app.thread_edit_message();
        assert!(matches!(
            app.popup(),
            Some(Popup::Compose(compose))
                if compose.target()
                    == &ComposeTarget::Edit {
                        thread: id.clone(),
                        message: MessageTarget::Reply(2),
                    }
        ));
        assert_eq!(app.compose_draft(), Some("reply from the list"));
        app.compose_cancel();

        let rows = app.thread_list_rows(100);
        let agent_row = rows
            .rows
            .iter()
            .position(|row| {
                matches!(
                    row,
                    Row::Message {
                        entry: 0,
                        message: 1,
                        ..
                    }
                )
            })
            .context("no agent message row after reply")?;
        app.thread_list_click(agent_row - app.thread_list().scroll());
        app.thread_open_in_file();
        assert_eq!(
            app.thread_panel().map(|_| app.thread_cursor().message()),
            Some(1),
            "Enter carries the selected message into the pane"
        );
        Ok(())
    }

    #[test]
    fn mouse_targets_the_pane_under_the_pointer() -> anyhow::Result<()> {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        use super::ThreadPane;
        use crate::app::Border;

        let mouse = |kind, column: usize, row: usize| MouseEvent {
            kind,
            column: u16::try_from(column).unwrap_or(u16::MAX),
            row: u16::try_from(row).unwrap_or(u16::MAX),
            modifiers: KeyModifiers::NONE,
        };
        let down = MouseEventKind::Down(MouseButton::Left);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);

        let dir = fixture("mouse")?;
        let mut app = app(&dir)?;
        app.start_comment();
        let long = (1..=20)
            .map(|n| format!("row {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        type_in(&mut app, &long);
        app.compose_submit();
        app.open_thread_at_cursor();
        assert_eq!(app.focus(), Focus::Thread);
        let rows = app.pane_rows();
        let top = rows - app.thread_rows();
        assert_eq!(app.text_rows(), top, "the pane takes rows from the text");
        app.thread_scroll(-100);

        // The wheel scrolls the pane under the pointer, focus aside.
        crate::app::input::mouse::handle_mouse(
            &mut app,
            mouse(MouseEventKind::ScrollDown, 20, top + 2),
        );
        assert_eq!(app.thread_panel().map(ThreadPane::scroll), Some(3));
        assert_eq!(app.view().scroll(), 0);
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 20, 0));
        assert_eq!(app.focus(), Focus::View, "a click on the text focuses it");
        assert!(app.thread_panel().is_some(), "the pane stays open");
        crate::app::input::mouse::handle_mouse(
            &mut app,
            mouse(MouseEventKind::ScrollUp, 20, top + 2),
        );
        assert_eq!(app.thread_panel().map(ThreadPane::scroll), Some(0));
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 20, top + 2));
        assert_eq!(app.focus(), Focus::Thread, "a click on the pane focuses it");

        // Dragging the rule resizes the pane.
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 20, top));
        assert_eq!(app.dragging(), Some(Border::Thread));
        crate::app::input::mouse::handle_mouse(&mut app, mouse(drag, 20, top - 4));
        assert_eq!(app.thread_rows(), rows - top + 4);
        assert_eq!(app.text_rows(), top - 4);
        crate::app::input::mouse::handle_mouse(&mut app, mouse(up, 20, top - 4));
        assert_eq!(app.dragging(), None);
        assert_eq!(app.view().selection(), None, "a border drag never selects");

        // Dragging the tree's divider resizes the tree.
        app.toggle_sidebar_focus();
        let width = app.sidebar_width();
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, width - 1, 3));
        assert_eq!(app.dragging(), Some(Border::Sidebar));
        crate::app::input::mouse::handle_mouse(&mut app, mouse(drag, 44, 3));
        assert_eq!(app.sidebar_width(), 45);
        crate::app::input::mouse::handle_mouse(&mut app, mouse(drag, 2, 3));
        assert_eq!(app.sidebar_width(), 8, "no narrower than the minimum");
        crate::app::input::mouse::handle_mouse(&mut app, mouse(up, 2, 3));
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 3, 0));
        assert_eq!(
            app.focus(),
            Focus::Sidebar,
            "the header row focuses the tree"
        );

        // The comment box keeps the keys but lets the mouse through.
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 20, 0));
        app.thread_reply();
        assert!(matches!(app.popup(), Some(Popup::Compose(_))));
        let top = rows - app.thread_rows();
        crate::app::input::mouse::handle_mouse(
            &mut app,
            mouse(MouseEventKind::ScrollDown, 20, top + 2),
        );
        assert_eq!(app.thread_panel().map(ThreadPane::scroll), Some(3));
        crate::app::input::mouse::handle_mouse(&mut app, mouse(down, 20, 0));
        assert!(
            matches!(app.popup(), Some(Popup::Compose(_))),
            "a click away leaves the box open"
        );
        Ok(())
    }

    #[test]
    fn the_status_line_badges_do_not_depend_on_focus() -> anyhow::Result<()> {
        let dir = fixture("status")?;
        let mut app = app(&dir)?;
        annotate(&mut app, "one")?;
        app.view_mut().toggle_source_view();
        let parts = crate::app::draw::status_parts(&app);
        assert_eq!(parts.pill, "NOR");
        assert_eq!(parts.badges, ["SRC"]);
        assert!(parts.right.contains("1 threads"), "{}", parts.right);
        app.open_thread_at_cursor();
        let parts = crate::app::draw::status_parts(&app);
        assert_eq!(parts.pill, "THREAD");
        assert_eq!(parts.badges, ["SRC"], "the badge outlives the focus change");
        Ok(())
    }

    /// Esc leaves a pane where it is; the Space key that opened it
    /// closes it (ADR 0010, amended 2026-09-03).
    #[test]
    fn esc_leaves_a_pane_and_its_space_key_closes_it() -> anyhow::Result<()> {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        use crate::app::input::keys;

        let dir = fixture("toggle")?;
        let mut app = app(&dir)?;
        annotate(&mut app, "one")?;
        let press = |app: &mut App, code| {
            keys::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
        };
        let space = |app: &mut App, ch| {
            press(app, KeyCode::Char(' '));
            press(app, KeyCode::Char(ch));
        };

        space(&mut app, 'a');
        assert_eq!(app.focus(), Focus::Thread);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.focus(), Focus::View);
        assert!(app.thread_panel().is_some(), "Esc leaves the pane open");
        space(&mut app, 'a');
        assert_eq!(
            app.focus(),
            Focus::Thread,
            "Space a returns to the open pane"
        );
        space(&mut app, 'a');
        assert!(
            app.thread_panel().is_none(),
            "Space a on the focused pane closes it"
        );
        assert_eq!(app.focus(), Focus::View);

        space(&mut app, 'A');
        assert!(app.thread_list().is_open());
        assert_eq!(app.focus(), Focus::Threads);
        space(&mut app, 'A');
        assert!(
            !app.thread_list().is_open(),
            "Space A on the focused list closes it"
        );
        space(&mut app, 'A');
        press(&mut app, KeyCode::Esc);
        assert!(
            !app.thread_list().is_open(),
            "Esc closes the list: it is the column"
        );
        assert_eq!(app.focus(), Focus::View);
        Ok(())
    }

    #[test]
    fn annotation_jumps_wrap_and_picker_lists_threads() -> anyhow::Result<()> {
        let dir = fixture("jumps")?;
        let mut app = app(&dir)?;
        app.thread_step_in_file(1);
        assert_eq!(app.message(), Some("no threads in this file"));
        app.start_comment();
        type_in(&mut app, "top");
        app.compose_submit();
        app.view_mut().goto_bottom();
        app.start_comment();
        type_in(&mut app, "bottom");
        app.compose_submit();
        let bottom = app.view().cursor_source_line();
        app.view_mut().goto_top();
        app.thread_step_in_file(1);
        assert_eq!(app.view().cursor_source_line(), bottom);
        app.thread_step_in_file(1);
        assert_eq!(app.view().cursor_source_line(), Some(1));
        assert_eq!(app.message(), Some("wrapped to first thread"));
        app.thread_step_in_file(-1);
        assert_eq!(app.view().cursor_source_line(), bottom);
        app.start_new_comment();
        app.compose_submit();
        assert_eq!(app.message(), Some("empty comment discarded"));
        Ok(())
    }

    /// ADR 0027: `c` on an annotated row opens the thread, `C` starts a
    /// second one there, and `n`/`p` in the pane walk the file's threads
    /// in line order with the cursor following.
    #[test]
    fn c_opens_the_thread_and_n_walks_the_file() -> anyhow::Result<()> {
        let dir = fixture("walk")?;
        let mut app = app(&dir)?;
        app.view_mut().goto_bottom();
        app.start_comment();
        type_in(&mut app, "bottom");
        app.compose_submit();
        let bottom = app.view().cursor_source_line();
        app.view_mut().goto_top();
        app.start_comment();
        type_in(&mut app, "top");
        app.compose_submit();
        assert!(app.thread_panel().is_none());

        // `c` again on the row opens the pane rather than a box.
        app.start_comment();
        assert!(app.popup().is_none());
        assert_eq!(app.focus(), Focus::Thread);
        assert_eq!(app.thread_position(), Some((1, 2)));
        app.close_thread();

        // `C` starts a second thread on the same line.
        app.start_new_comment();
        assert!(
            matches!(app.popup(), Some(Popup::Compose(c)) if matches!(c.target(), ComposeTarget::New(_)))
        );
        type_in(&mut app, "top again");
        app.compose_submit();
        assert_eq!(app.thread_counts(), (3, 3));

        // The walk is by line, then store order, and moves the cursor.
        app.open_thread_at_cursor();
        assert_eq!(app.thread_position(), Some((1, 3)));
        app.thread_step_in_file(1);
        assert_eq!(app.thread_position(), Some((2, 3)));
        assert_eq!(app.view().cursor_source_line(), Some(1));
        app.thread_step_in_file(1);
        assert_eq!(app.thread_position(), Some((3, 3)));
        assert_eq!(app.view().cursor_source_line(), bottom);
        app.thread_step_in_file(1);
        assert_eq!(app.thread_position(), Some((1, 3)), "wraps");
        assert_eq!(app.view().cursor_source_line(), Some(1));
        app.thread_step_in_file(-1);
        assert_eq!(app.view().cursor_source_line(), bottom);
        Ok(())
    }

    #[test]
    fn the_thread_list_shows_the_work_and_acts_in_place() -> anyhow::Result<()> {
        let dir = fixture("list")?;
        let mut app = app(&dir)?;
        app.start_comment();
        type_in(&mut app, "top");
        app.compose_submit();
        app.view_mut().goto_bottom();
        app.start_comment();
        type_in(&mut app, "bottom");
        app.compose_submit();
        let bottom = app.view().cursor_source_line();
        app.view_mut().goto_top();

        // The list (ADR 0025) takes the column: both threads, open first,
        // under the file; Enter jumps to the selected one and opens the pane.
        app.open_thread_list();
        assert_eq!(app.focus(), Focus::Threads);
        assert!(app.thread_panel().is_none());
        let rows = app.thread_list_rows(60);
        assert_eq!(rows.entries.len(), 2);
        assert!(matches!(
            rows.rows.first(),
            Some(Row::Section {
                resolved: false,
                count: 2,
                ..
            })
        ));
        assert!(matches!(&rows.rows[1], Row::File(path) if path == Path::new("README.md")));
        assert!(
            rows.rows
                .iter()
                .any(|row| matches!(row, Row::Body { text, .. } if text.trim() == "top"))
        );
        app.thread_list_step(1);
        app.thread_open_in_file();
        assert!(!app.thread_list().is_open());
        assert_eq!(app.focus(), Focus::Thread);
        assert!(app.thread_panel().is_some());
        assert_eq!(app.view().cursor_source_line(), bottom);

        // `x` moves an entry to the resolved section; `f` narrows to the
        // file; `Z` folds the resolved section; reopening keeps the entry.
        app.open_thread_list();
        app.thread_toggle_resolved();
        let rows = app.thread_list_rows(60);
        assert!(matches!(
            rows.rows.first(),
            Some(Row::Section {
                resolved: false,
                count: 1,
                ..
            })
        ));
        assert!(rows.rows.iter().any(|row| matches!(
            row,
            Row::Section {
                resolved: true,
                count: 1,
                ..
            }
        )));
        assert_eq!(app.message(), Some("resolved"));
        app.thread_list_fold_resolved();
        let rows = app.thread_list_rows(60);
        assert_eq!(rows.entries.len(), 2);
        assert_eq!(
            rows.rows
                .iter()
                .filter(|row| matches!(row, Row::File(_)))
                .count(),
            1
        );
        app.thread_list_fold_resolved();
        app.thread_list_toggle_file();
        assert_eq!(app.thread_list_rows(60).entries.len(), 2);
        app.thread_list_toggle_file();
        app.close_thread_list();
        assert_eq!(app.focus(), Focus::View);
        app.open_thread_list();
        app.thread_reply();
        type_in(&mut app, "still here");
        app.compose_submit();
        assert_eq!(app.focus(), Focus::Threads);
        assert!(app.thread_list().is_open());
        assert!(
            app.thread_list_rows(60)
                .rows
                .iter()
                .any(|row| matches!(row, Row::Body { text, .. } if text.trim() == "still here"))
        );
        // A file opened by any route takes the column back.
        app.open(Path::new("README.md"));
        assert!(!app.thread_list().is_open());
        assert_eq!(app.focus(), Focus::View);

        Ok(())
    }

    #[test]
    fn socket_requests_open_follow_list_and_reply() -> anyhow::Result<()> {
        let dir = fixture("socket")?;
        fs::write(dir.0.join("ws/other.md"), "# Other\n\nline\n")?;
        let mut app = app(&dir)?;
        app.view_mut().move_down(2);
        app.view_mut().select_lines();
        app.start_comment();
        type_in(&mut app, "please check");
        app.compose_submit();
        let id = app.marks()[0].id().clone();

        // Paths must stay inside the workspace.
        for bad in ["../ws/README.md", "/etc/passwd", "missing.md"] {
            let reply = app.handle_request(Request::Open {
                path: PathBuf::from(bad),
                line: None,
                end_line: None,
            });
            assert!(matches!(reply, Response::Error(_)), "{bad}: {reply:?}");
        }
        let reply = app.handle_request(Request::Open {
            path: PathBuf::from("other.md"),
            line: Some(3),
            end_line: Some(3),
        });
        assert_eq!(reply, Response::Done);
        assert_eq!(app.current_path(), Path::new("other.md"));
        assert_eq!(app.view().cursor_source_line(), Some(3));

        assert_eq!(
            app.handle_request(Request::Follow {
                paths: vec![PathBuf::from("other.md")]
            }),
            Response::Done
        );
        assert_eq!(app.followed(), [PathBuf::from("other.md")]);

        let Response::Threads(all) = app.handle_request(Request::AnnotationsList {
            since: None,
            path: None,
        }) else {
            anyhow::bail!("no thread list");
        };
        assert_eq!(all.len(), 1);
        let Response::Threads(none) = app.handle_request(Request::AnnotationsList {
            since: Some(all[0].updated() + 1),
            path: None,
        }) else {
            anyhow::bail!("no thread list");
        };
        assert!(none.is_empty());
        let Response::Threads(elsewhere) = app.handle_request(Request::AnnotationsList {
            since: None,
            path: Some(PathBuf::from("other.md")),
        }) else {
            anyhow::bail!("no thread list");
        };
        assert!(elsewhere.is_empty());

        let author = Author::Agent {
            name: "reviewer".to_owned(),
            client: Some("claude-code".to_owned()),
            id: None,
            kind: None,
        };
        let reply = app.handle_request(Request::ThreadReply {
            thread: id.clone(),
            author: author.clone(),
            body: "fixed".to_owned(),
            resolve: true,
            lines: None,
        });
        assert_eq!(reply, Response::Done);
        assert_eq!(
            app.toasts().last().map(crate::app::Toast::text),
            Some("reply on README.md:3, resolved")
        );
        let thread = app
            .thread(&id)
            .ok_or_else(|| anyhow::anyhow!("thread lost"))?;
        assert_eq!(thread.status(), Status::AutoResolved);
        assert_eq!(thread.replies()[0].author(), &author);
        assert!(thread.replies()[0].proposes_resolution());
        assert_eq!(
            thread.replies()[0].author().to_string(),
            "reviewer (claude-code)"
        );
        app.open(Path::new("README.md"));
        assert_eq!(app.thread_counts(), (0, 1));

        let reply = app.handle_request(Request::ThreadReply {
            thread: serde_json::from_str(r#""9-9-9""#)?,
            author,
            body: "?".to_owned(),
            resolve: false,
            lines: None,
        });
        assert!(matches!(reply, Response::Error(message) if message.contains("unknown thread")));
        Ok(())
    }

    fn draft(app: &App) -> anyhow::Result<(String, Cursor)> {
        let Some(Popup::Compose(compose)) = app.popup() else {
            anyhow::bail!("comment box is not open");
        };
        Ok((
            compose.buffer().text().to_owned(),
            compose.buffer().cursor(),
        ))
    }

    #[test]
    fn the_comment_box_edits_around_a_cursor_and_takes_pastes() -> anyhow::Result<()> {
        let dir = fixture("editor")?;
        let mut app = app(&dir)?;
        app.view_mut().select_lines();
        app.start_comment();
        type_in(&mut app, "second\nfourth");
        app.compose_edit(Edit::Move(Motion::Up));
        app.compose_edit(Edit::Move(Motion::LineStart));
        app.compose_insert("first ");
        app.paste("\r\nthird\r\n");
        assert_eq!(
            draft(&app)?,
            (
                "first \nthird\nsecond\nfourth".to_owned(),
                Cursor { line: 2, column: 0 }
            )
        );
        app.compose_edit(Edit::DeleteWordBack);
        assert_eq!(draft(&app)?.0, "first \nsecond\nfourth");
        // The editor hatch round-trips the whole draft, cursor at the end.
        assert_eq!(app.compose_draft(), Some("first \nsecond\nfourth"));
        app.set_compose_text("from the editor\nline two");
        assert_eq!(
            draft(&app)?,
            (
                "from the editor\nline two".to_owned(),
                Cursor { line: 1, column: 8 }
            )
        );
        // The box is two rows over the rule and header until dragged; with
        // a 100-column pane nothing wraps.
        assert_eq!(app.compose_rows(), 4);
        assert_eq!(app.compose_first_row(), 0);
        app.compose_submit();
        assert!(app.popup().is_none());
        Ok(())
    }

    #[test]
    fn esc_asks_twice_before_discarding_a_draft() -> anyhow::Result<()> {
        let dir = fixture("discard")?;
        let mut app = app(&dir)?;
        app.view_mut().select_lines();
        app.start_comment();
        app.compose_cancel();
        assert!(app.popup().is_none(), "an empty box closes at once");
        app.start_comment();
        type_in(&mut app, "keep me");
        app.compose_cancel();
        assert_eq!(app.message(), Some("Esc again to discard the comment"));
        // Typing keeps the draft and drops the prompt.
        type_in(&mut app, "!");
        let Some(Popup::Compose(compose)) = app.popup() else {
            anyhow::bail!("draft was lost");
        };
        assert!(!compose.confirming_discard());
        assert_eq!(compose.buffer().text(), "keep me!");
        app.compose_cancel();
        app.compose_cancel();
        assert!(app.popup().is_none());
        assert_eq!(app.thread_counts(), (0, 0));
        Ok(())
    }

    #[test]
    fn ctrl_c_clears_the_draft_and_closes_an_empty_box() -> anyhow::Result<()> {
        let dir = fixture("clear")?;
        let mut app = app(&dir)?;
        app.view_mut().select_lines();
        app.start_comment();
        app.compose_clear();
        assert!(app.popup().is_none(), "an empty box closes at once");
        app.start_comment();
        type_in(&mut app, "wipe me");
        app.compose_clear();
        assert_eq!(draft(&app)?.0, "", "the box stays open, emptied");
        app.compose_clear();
        assert!(app.popup().is_none());
        assert_eq!(app.thread_counts(), (0, 0));
        Ok(())
    }

    #[test]
    fn a_long_comment_wraps_and_scrolls_to_the_cursor() -> anyhow::Result<()> {
        let dir = fixture("wrap")?;
        let mut app = app(&dir)?;
        app.view_mut().select_lines();
        app.start_comment();
        let width = app.compose_width();
        type_in(&mut app, &"x".repeat(width * 10));
        // Ten wrapped rows exceed the cap: eight rows, cursor row last.
        assert_eq!(app.compose_rows(), 8);
        assert_eq!(app.compose_first_row(), 4);
        app.compose_edit(Edit::Move(Motion::Up));
        assert_eq!(app.compose_first_row(), 0);
        // A click lands on the wrapped cell under the pointer; dragging the
        // box's rule gives it the rows the pointer leaves below.
        app.compose_click(2, 6);
        assert_eq!(
            draft(&app)?.1,
            Cursor {
                line: 0,
                column: width * 2 + 5
            }
        );
        app.begin_drag(Border::Compose);
        app.drag_to(0, app.pane_rows() - 12);
        app.end_drag();
        assert_eq!(app.compose_rows(), 12);
        Ok(())
    }

    /// Every pane, popup and overlay draws at any terminal size a terminal
    /// emulator can report, down to a single cell.
    #[test]
    fn every_overlay_draws_at_any_terminal_size() -> anyhow::Result<()> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let dir = fixture("sizes")?;
        let mut app = app(&dir)?;
        app.view_mut().move_down(2);
        app.view_mut().select_lines();
        app.start_comment();
        type_in(&mut app, "a question about this line");
        app.compose_submit();
        let id = app.marks()[0].id().clone();
        app.agent_reply(
            &id,
            Author::Agent {
                name: "Copilot".to_owned(),
                client: None,
                id: None,
                kind: None,
            },
            (1..=12)
                .map(|n| format!("line {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            true,
            None,
        )
        .map_err(anyhow::Error::msg)?;

        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::draw::Theme::from_core(&core);
        let draw = |app: &mut App, state: &str| -> anyhow::Result<()> {
            for width in [1u16, 2, 4, 8, 12, 20, 40, 80] {
                for height in 1..=6u16 {
                    app.resize(usize::from(width), usize::from(height));
                    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
                    terminal
                        .draw(|frame| crate::app::draw::draw(frame, app, &theme))
                        .with_context(|| format!("{state} at {width}x{height}"))?;
                }
            }
            app.resize(100, 30);
            Ok(())
        };

        draw(&mut app, "text")?;
        app.show_sidebar();
        draw(&mut app, "sidebar")?;
        app.open_thread(id);
        draw(&mut app, "thread")?;
        app.thread_reply();
        type_in(&mut app, "a reply long enough to wrap more than once over");
        draw(&mut app, "compose over thread")?;
        app.close_popup();
        app.open_help();
        draw(&mut app, "help")?;
        app.close_popup();
        app.open_status();
        draw(&mut app, "status")?;
        app.close_popup();
        app.open_picker(crate::app::PickerKind::Files);
        draw(&mut app, "picker")?;
        app.close_popup();
        app.open_thread_list();
        draw(&mut app, "thread list")?;
        app.thread_reply();
        type_in(&mut app, "a reply from the list");
        draw(&mut app, "compose over list")?;
        Ok(())
    }
}
