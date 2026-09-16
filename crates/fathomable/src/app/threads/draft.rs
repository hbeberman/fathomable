// @okf-doc: /decisions/0054-the-draft-is-written-in-the-thread.md
//! The draft (ADR 0054): a comment, reply, or edit written in the rows
//! of the text, in the thread it belongs to.
//!
//! A reply is written at the bottom of its thread's expanded block, an
//! edit in place of the message it edits, and a new comment in a draft
//! block under its lines. The stubs (`stubs`) give the draft its rows;
//! this module opens and closes it, edits its [`Buffer`] (ADR 0018),
//! says where its rows and its cursor are, and keeps that cursor on
//! screen. The review list opens the file to write and comes back when
//! the draft closes. [`Popup::Compose`] is only the state that routes
//! the keys here; nothing pops up. Leaving a file parks its draft in
//! the document; showing that document again restores it.

use fathomable_core::annotations::{Author, Draft, LineRange, MessageTarget, Reply, ThreadId};
use fathomable_core::clock::now;
use fathomable_core::content::Content;
use fathomable_core::editor::{Buffer, Cell, Edit};

use crate::app::draw::message::MESSAGE_INDENT;
use crate::app::{App, Popup};

#[cfg(test)]
mod tests;

/// What the draft will produce on submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ComposeTarget {
    /// A new thread on these source lines.
    New(LineRange),
    /// A new thread on the open file as a whole (ADR 0063).
    OnFile,
    /// A reply to an existing thread.
    Reply(ThreadId),
    /// A replacement for one user-authored message.
    Edit {
        thread: ThreadId,
        message: MessageTarget,
    },
}

impl ComposeTarget {
    /// The thread the draft is written in, `None` for a new comment.
    #[must_use]
    pub(crate) fn thread(&self) -> Option<&ThreadId> {
        match self {
            Self::New(_) | Self::OnFile => None,
            Self::Reply(id) | Self::Edit { thread: id, .. } => Some(id),
        }
    }

    /// The message an edit replaces, as the thread counts them: zero the
    /// comment, then the replies.
    #[must_use]
    pub(crate) fn edited_message(&self) -> Option<usize> {
        match self {
            Self::Edit { message, .. } => Some(match message {
                MessageTarget::Comment => 0,
                MessageTarget::Reply(index) => index + 1,
            }),
            Self::New(_) | Self::OnFile | Self::Reply(_) => None,
        }
    }
}

/// The multi-line draft (ADR 0005, 0018), written in the text (ADR 0054).
#[derive(Debug)]
pub(crate) struct Compose {
    target: ComposeTarget,
    buffer: Buffer,
    /// The text the draft opened with, empty for a new comment or reply.
    original: String,
    /// Esc was pressed on a non-empty draft; the next Esc discards it.
    confirm_discard: bool,
    /// The review list was showing when the draft opened; it comes back
    /// when the draft closes.
    from_review: bool,
}

impl Compose {
    pub(crate) fn target(&self) -> &ComposeTarget {
        &self.target
    }

    /// The draft and its cursor (ADR 0018).
    pub(crate) fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Whether the draft is asking for a second Esc.
    pub(crate) fn confirming_discard(&self) -> bool {
        self.confirm_discard
    }
}

/// Which of a draft's rows a rendered row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DraftRow {
    /// The ` User  draft` row with the draft keys at its right edge.
    Author,
    /// A wrapped row of the draft's text, counted from its first.
    Text(usize),
}

impl App {
    /// `c`: reply from a thread row, else comment on the selection or line.
    pub(crate) fn start_comment(&mut self) {
        if self.view().selected_lines().is_none()
            && let Some(id) = self.thread_cursor().thread().cloned()
            && self.cursor_on_thread_row(&id)
        {
            self.open_compose(ComposeTarget::Reply(id));
            return;
        }
        self.start_new_comment();
    }

    /// Whether the open file can take a comment: there is one, it is
    /// text (ADR 0026), and the store is open; says why not otherwise.
    pub(super) fn can_annotate(&mut self) -> bool {
        let Some(doc) = self.current.and_then(|index| self.docs.get(index)) else {
            self.notice("open a file to annotate it");
            return false;
        };
        // Threads anchor to lines, and these files have none (ADR 0026).
        match doc.document.content() {
            Content::Text(_) => {}
            Content::Binary { .. } => {
                self.notice("cannot annotate a binary file");
                return false;
            }
            Content::TooLarge { .. } => {
                self.notice("cannot annotate a file this large");
                return false;
            }
        }
        if self.store.is_none() {
            self.store_mut();
            return false;
        }
        true
    }

    /// Start a draft on the selection or cursor line.
    pub(crate) fn start_new_comment(&mut self) {
        if !self.can_annotate() {
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

    /// Open a draft for `target` in the text: the review list closes if
    /// it was showing, the thread's file opens and the thread expands
    /// with the cursor on the message the draft answers or edits, and
    /// the draft's rows join the block (ADR 0054).
    pub(super) fn open_compose(&mut self, target: ComposeTarget) {
        // A new thread would anchor to a snapshot no file matches, and a
        // reply would land on one (ADR 0028).
        let deleted = match &target {
            ComposeTarget::New(_) | ComposeTarget::OnFile => self
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
        // A thread resolved at an earlier commit is not written in from
        // the past: `o` brings it back first (ADR 0072).
        if let ComposeTarget::Reply(id) | ComposeTarget::Edit { thread: id, .. } = &target
            && let Some(commit) = self
                .thread(id)
                .filter(|thread| self.reach.past(thread))
                .and_then(|thread| thread.commit())
        {
            let commit = crate::app::threads::list::short_commit(commit);
            self.notice(format!("resolved at {commit}; o reopens it"));
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
            ComposeTarget::New(_) | ComposeTarget::OnFile | ComposeTarget::Reply(_) => {
                String::new()
            }
        };
        let from_review = self.review_list.is_open();
        if from_review {
            self.close_review();
        }
        // The draft is written in the thread's rows, so the thread shows
        // expanded with the cursor on the message it answers or edits.
        if let Some(id) = target.thread().cloned() {
            let elsewhere = self
                .thread(&id)
                .is_some_and(|thread| thread.path() != self.current_path());
            if elsewhere && !self.land_on_thread(id.clone()) {
                return;
            }
            let message = target
                .edited_message()
                .unwrap_or_else(|| self.newest_message(&id));
            self.goto_message(id, message);
        }
        self.resume_draft();
        if self.draft().is_some() {
            self.notice("finish or discard this file's draft first");
            return;
        }
        let buffer = Buffer::from_text(&original);
        self.popup = Some(Popup::Compose(Compose {
            target,
            buffer,
            original,
            confirm_discard: false,
            from_review,
        }));
        self.draft_changed();
    }

    /// Keep the active draft with its document before leaving it.
    pub(in crate::app) fn park_draft(&mut self) {
        if let Some(index) = self.current
            && matches!(self.popup, Some(Popup::Compose(_)))
            && let Some(Popup::Compose(compose)) = self.popup.take()
        {
            self.docs[index].draft = Some(compose);
            self.place_stub_rows();
        }
    }

    /// Give the current document's waiting draft its rows and keys back.
    pub(in crate::app) fn resume_draft(&mut self) {
        if self.popup.is_some() {
            return;
        }
        if let Some(index) = self.current
            && let Some(compose) = self.docs[index].draft.take()
        {
            if let Some(id) = compose.target.thread() {
                self.expanded.insert(id.clone());
            }
            self.popup = Some(Popup::Compose(compose));
            self.draft_changed();
        }
    }

    /// The open draft, if one is.
    pub(crate) fn draft(&self) -> Option<&Compose> {
        match &self.popup {
            Some(Popup::Compose(compose)) => Some(compose),
            _ => None,
        }
    }

    /// The open draft, its discard prompt cleared: any edit means the
    /// draft is being kept.
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
            self.draft_changed();
        }
    }

    /// A motion or deletion in the draft (ADR 0018).
    pub(crate) fn compose_edit(&mut self, edit: Edit) {
        if let Some(compose) = self.compose_mut() {
            compose.buffer.apply(edit);
            self.draft_changed();
        }
    }

    /// The draft's text, for the `$EDITOR` hatch.
    pub(crate) fn compose_draft(&self) -> Option<&str> {
        self.draft().map(|compose| compose.buffer.text())
    }

    /// Replace the whole draft (back from `$EDITOR`), cursor at the end.
    pub(crate) fn set_compose_text(&mut self, text: &str) {
        if let Some(compose) = self.compose_mut() {
            compose.buffer = Buffer::from_text(text);
            self.draft_changed();
        }
    }

    /// Alt-Up / Alt-Down while writing: scroll the text, where the thread
    /// is expanded (ADR 0049), to read what is above the draft.
    pub(crate) fn compose_scroll(&mut self, delta: isize) {
        if self.draft().is_some() {
            self.view_mut().scroll_by(delta);
        }
    }

    /// Esc: drop the draft. A changed draft or edit asks for a second
    /// Esc first.
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
        self.discard_draft();
        self.notice(if editing {
            "edit cancelled"
        } else {
            "comment cancelled"
        });
    }

    /// Ctrl-C: wipe a non-empty draft in place; an empty draft closes.
    pub(crate) fn compose_clear(&mut self) {
        let Some(Popup::Compose(compose)) = self.popup.as_mut() else {
            return;
        };
        let editing = matches!(compose.target, ComposeTarget::Edit { .. });
        if compose.buffer.text().trim().is_empty() {
            self.discard_draft();
            self.notice(if editing {
                "edit cancelled"
            } else {
                "comment cancelled"
            });
            return;
        }
        compose.buffer = Buffer::new();
        compose.confirm_discard = false;
        self.draft_changed();
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
            self.discard_draft();
            self.notice("empty comment discarded");
            return;
        }
        let Some(Popup::Compose(compose)) = self.popup.take() else {
            return;
        };
        match compose.target {
            ComposeTarget::New(range) => self.submit_annotation(Some(range), text),
            ComposeTarget::OnFile => self.submit_annotation(None, text),
            ComposeTarget::Reply(id) => self.submit_reply(&id, text),
            ComposeTarget::Edit { thread, message } => {
                self.submit_message_edit(&thread, message, text);
            }
        }
        self.draft_closed(compose.from_review);
    }

    /// Drop the draft without writing anything.
    fn discard_draft(&mut self) {
        if let Some(Popup::Compose(compose)) = self.popup.take() {
            self.draft_closed(compose.from_review);
        }
    }

    /// The draft closed: its rows go, and the review list comes back
    /// with the keys when it was showing. From the threads pane the keys
    /// never left it (ADR 0034).
    fn draft_closed(&mut self, from_review: bool) {
        // A new message changes what the stubs show (ADR 0049).
        self.place_stub_rows();
        if from_review {
            self.open_review();
        }
    }

    /// The draft's text changed: its rows are laid out again and the
    /// view scrolls to show the row its cursor is on, above the key bar
    /// that covers the bottom text row while the draft is open (ADR
    /// 0067).
    fn draft_changed(&mut self) {
        self.place_stub_rows();
        if let Some((row, _)) = self.draft_cursor_cell() {
            self.view_mut().reveal_row(row, 0);
        }
    }

    // ----- the draft's place in the rows -----

    /// Columns the draft wraps at: the text width less the message
    /// indent, as a message body wraps.
    pub(crate) fn draft_width(&self) -> usize {
        self.view()
            .layout()
            .width()
            .saturating_sub(MESSAGE_INDENT)
            .max(1)
    }

    /// Rows the draft takes in its block: the author row and its wrapped
    /// text, one row at least.
    pub(crate) fn draft_rows(&self) -> usize {
        self.draft().map_or(0, |compose| {
            1 + compose.buffer().rows(self.draft_width()).len().max(1)
        })
    }

    /// The block holding the draft and the block row of its author row.
    fn draft_slot(&self) -> Option<(usize, usize)> {
        self.stubs()
            .iter()
            .enumerate()
            .find_map(|(block, stub)| stub.draft_row().map(|row| (block, row)))
    }

    /// The rendered row and the column from the gutter the draft's
    /// cursor is on, while the draft has rows.
    pub(crate) fn draft_cursor_cell(&self) -> Option<(usize, usize)> {
        let compose = self.draft()?;
        let (block, author) = self.draft_slot()?;
        let cell = compose.buffer().cursor_cell(self.draft_width());
        let row = self.view().row_of_stub_slot(block, author + 1 + cell.row)?;
        Some((row, MESSAGE_INDENT + cell.column))
    }

    /// The rendered row of the draft's author row.
    #[cfg(test)]
    pub(crate) fn draft_author_row(&self) -> Option<usize> {
        let (block, author) = self.draft_slot()?;
        self.view().row_of_stub_slot(block, author)
    }

    /// Which of the draft's rows rendered row `row` is, `None` off them.
    pub(crate) fn draft_row_of(&self, row: usize) -> Option<DraftRow> {
        let (stub, index, _) = self.stub_on_row(row)?;
        let author = stub.draft_row()?;
        match index.checked_sub(author) {
            Some(0) => Some(DraftRow::Author),
            Some(offset) => Some(DraftRow::Text(offset - 1)),
            None => None,
        }
    }

    /// A click on wrapped row `row` of the draft's text, `column` cells
    /// from the gutter, moves the cursor there.
    pub(crate) fn draft_place_cursor(&mut self, row: usize, column: usize) {
        let width = self.draft_width();
        let cell = Cell {
            row,
            column: column.saturating_sub(MESSAGE_INDENT),
        };
        if let Some(compose) = self.compose_mut() {
            compose.buffer.place_cursor(width, cell);
        }
    }

    // ----- writing to the store -----

    /// Start the thread on `range` of the open file, or on the file as a
    /// whole with no range (ADR 0063).
    fn submit_annotation(&mut self, range: Option<LineRange>, comment: String) {
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
        let draft = match range {
            Some(range) => Draft::new(Author::User, &path, range, comment),
            None => Draft::on_file(Author::User, &path, comment),
        }
        .at_commit(self.workspace.head_commit());
        let where_at = range.map_or_else(|| "the file".to_owned(), |range| format!("L{range}"));
        let Some(store) = self.store_mut() else {
            return;
        };
        match store.annotate(draft, &text, now()) {
            Ok(id) => {
                tracing::info!(%id, path = %path.display(), %where_at, "thread started");
                self.refresh_reach();
                self.refresh_marks(index);
                // Commenting on lines means they were read (ADR 0020).
                self.mark_seen(index);
                self.view_mut().clear_selection();
                self.notice(format!("commented on {where_at}"));
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
                // The reply becomes the highlighted message, under the
                // draft's rows it replaces (ADR 0049).
                let newest = self.newest_message(id);
                self.goto_message(id.clone(), newest);
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
}
