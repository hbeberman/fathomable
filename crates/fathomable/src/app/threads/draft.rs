// @okf-doc: /decisions/0054-the-draft-is-written-in-the-thread.md
//! The draft (ADR 0054): a comment, reply, or edit written in its
//! conversation, inline or in the Review entry when source is absent.
//!
//! A reply is written at the bottom of its thread's expanded block, an
//! edit in place of the message it edits, and a new comment in a draft
//! block under its lines. The stubs (`stubs`) give the draft its rows;
//! this module opens and closes it, edits its [`Buffer`] (ADR 0018),
//! says where its rows and its cursor are, and keeps that cursor on
//! screen. The review list opens the file when it can and otherwise
//! hosts a conversation-only draft. [`Popup::Compose`] is only the
//! state that routes the keys here; nothing pops up. Leaving a file
//! parks its draft in the document; showing that document again
//! restores it.

use std::path::PathBuf;
use std::sync::Arc;

use fathomable_core::annotations::{
    Author, ComparisonFacts, ContentIdentity, Draft, FullFileDigest, IndexFacts, IndexState,
    LineRange, MessageTarget, OriginSide, OriginVersion, Provenance, ReviewPointFacts, ThreadId,
    UserSubmit, UserWriteOutcome, WorkingTreeFacts, WorkingTreeState,
};
use fathomable_core::clock::now;
use fathomable_core::content::Content;
use fathomable_core::editor::{Buffer, Cell, Edit};

use crate::app::draw::message::MESSAGE_INDENT;
use crate::app::threads::list::ReviewView;
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
    /// Immutable displayed evidence captured when a new annotation opens.
    annotation: Option<Arc<AnnotationEvidence>>,
    buffer: Buffer,
    /// The text the draft opened with, empty for a new comment or reply.
    original: String,
    /// Esc was pressed on a non-empty draft; the next Esc discards it.
    confirm_discard: bool,
    /// Submission paused because another writer resolved the thread.
    confirm_reopen: Option<UserSubmit>,
    /// The review view that was showing when the draft opened.
    from_review: Option<ReviewView>,
}

#[derive(Debug, Clone)]
struct AnnotationEvidence {
    path: PathBuf,
    range: Option<LineRange>,
    text: String,
    side: OriginSide,
    provenance: Provenance,
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

    /// Whether Enter will atomically reopen and submit this intact draft.
    pub(crate) fn confirming_reopen(&self) -> bool {
        self.confirm_reopen.is_some()
    }

    pub(in crate::app) fn rename_annotation_path(&mut self, path: &std::path::Path) {
        if let Some(evidence) = self.annotation.as_mut() {
            Arc::make_mut(evidence).path = path.to_path_buf();
        }
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
    /// Whether a new line or file annotation is active or parked.
    pub(in crate::app) fn has_new_annotation_draft(&self) -> bool {
        let is_new = |compose: &Compose| {
            matches!(
                compose.target(),
                ComposeTarget::New(_) | ComposeTarget::OnFile
            )
        };
        matches!(&self.popup, Some(Popup::Compose(compose)) if is_new(compose))
            || self
                .docs
                .iter()
                .filter_map(|doc| doc.draft.as_ref())
                .any(is_new)
    }

    /// Block endpoint and mode changes while a new annotation is pending.
    pub(in crate::app) fn annotation_draft_blocks(&mut self, action: &str) -> bool {
        if self.has_new_annotation_draft() {
            self.notice(format!(
                "submit or cancel the pending annotation before {action}"
            ));
            return true;
        }
        false
    }

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
        if !self.annotation_projection_matches_selection() {
            self.notice("comparison projection is not ready; cannot annotate displayed content");
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
        if let Some(selection) = view.selection()
            && mixed_diff_selection(view, selection)
        {
            self.notice("select one diff side before commenting");
            return;
        }
        if cursor_on_removed_diff_line(view) && displayed_diff_side_and_range(view).is_none() {
            self.notice("cannot determine the removed line's source evidence");
            return;
        }
        // A detached thread's row is not text (ADR 0039).
        let on_detached_row = view.selected_lines().is_none()
            && view.detached_anchor_of_row(view.cursor().row).is_some();
        let range = displayed_diff_range(view)
            .or_else(|| view.selected_lines())
            .or_else(|| {
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
        // New annotations require displayed source. Existing conversations
        // retain their immutable identity and lifecycle without a placement.
        let deleted = match &target {
            ComposeTarget::New(_) | ComposeTarget::OnFile => self
                .current
                .and_then(|index| self.docs.get(index))
                .filter(|doc| {
                    doc.deleted.is_some()
                        && doc.deleted != Some(crate::app::Deleted::ComparisonBase)
                })
                .map(|doc| doc.relative.clone()),
            ComposeTarget::Reply(_) | ComposeTarget::Edit { .. } => None,
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
            ComposeTarget::New(_) | ComposeTarget::OnFile | ComposeTarget::Reply(_) => {
                String::new()
            }
        };
        let annotation = match &target {
            ComposeTarget::New(range) => self.annotation_evidence(Some(*range)).map(Arc::new),
            ComposeTarget::OnFile => self.annotation_evidence(None).map(Arc::new),
            ComposeTarget::Reply(_) | ComposeTarget::Edit { .. } => None,
        };
        if matches!(target, ComposeTarget::New(_) | ComposeTarget::OnFile) && annotation.is_none() {
            self.notice("no open file");
            return;
        }
        let from_review = self.review_list.is_open().then_some(self.review.view);
        if from_review.is_some() {
            self.close_review();
        }
        // The draft is written in the thread's rows, so the thread shows
        // expanded with the cursor on the message it answers or edits.
        if let Some(id) = target.thread().cloned() {
            if !self.thread_source_is_displayable(&id) {
                match self.land_on_thread(id.clone()) {
                    Some(
                        crate::app::threads::cursor::ThreadLanding::Source
                        | crate::app::threads::cursor::ThreadLanding::Review,
                    ) => {}
                    None => return,
                }
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
        if matches!(target, ComposeTarget::New(_) | ComposeTarget::OnFile) {
            self.comparison.freeze_for_annotation();
        }
        let buffer = Buffer::from_text(&original);
        self.popup = Some(Popup::Compose(Compose {
            target,
            annotation,
            buffer,
            original,
            confirm_discard: false,
            confirm_reopen: None,
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

    /// Give the current document's parked draft its rows and keys back.
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
                compose.confirm_reopen = None;
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
        if compose.confirm_reopen.take().is_some() {
            self.notice("continued editing; the thread remains resolved");
            return;
        }
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
        self.compose_submit_with(UserSubmit::Normal);
    }

    /// Ctrl-Enter: submit or save and enable one-shot auto-resolve.
    pub(crate) fn compose_submit_auto_resolve(&mut self) {
        self.compose_submit_with(UserSubmit::EnableAutoResolve);
    }

    fn compose_submit_with(&mut self, requested: UserSubmit) {
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
        let target = compose.target.clone();
        let annotation = compose.annotation.clone();
        let from_review = compose.from_review;
        let submission = compose.confirm_reopen.unwrap_or(requested);
        let reopen = compose.confirm_reopen.is_some();
        let result = match target {
            ComposeTarget::New(_) | ComposeTarget::OnFile => {
                let Some(evidence) = annotation else {
                    self.error("annotation evidence is unavailable");
                    return;
                };
                self.submit_annotation(&evidence, text, submission)
            }
            ComposeTarget::Reply(id) => self.submit_reply(&id, text, submission, reopen),
            ComposeTarget::Edit { thread, message } => {
                self.submit_message_edit(&thread, message, text, submission, reopen)
            }
        };
        match result {
            Ok(SubmitResult::Applied) => {
                self.popup = None;
                self.draft_closed(from_review);
            }
            Ok(SubmitResult::ReopenRequired) => {
                if let Some(Popup::Compose(compose)) = self.popup.as_mut() {
                    compose.confirm_reopen = Some(submission);
                }
                self.notice("thread was resolved while editing; Enter reopens and submits");
            }
            Err(error) => self.error(error),
        }
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
    fn draft_closed(&mut self, from_review: Option<ReviewView>) {
        // A new message changes what the stubs show (ADR 0049).
        self.place_stub_rows();
        if let Some(view) = from_review {
            self.open_review_view(view);
        }
        if !self.has_new_annotation_draft() && self.comparison.unfreeze_after_annotation() {
            self.refresh_comparison();
        }
        self.refresh_all_marks();
        self.reconcile_normal_thread_cursor();
    }

    /// The draft's text changed: its rows are laid out again and the
    /// view scrolls to show the row its cursor is on, above the key bar
    /// that covers the bottom text row while the draft is open (ADR
    /// 0067).
    fn draft_changed(&mut self) {
        self.place_stub_rows();
        if self.review_list.is_open() {
            self.review_follow_draft();
        } else if let Some((row, _)) = self.draft_cursor_cell() {
            self.view_mut().reveal_row(row, 0);
        }
    }

    // ----- the draft's place in the rows -----

    /// Columns the draft wraps at: the text width less the message
    /// indent, as a message body wraps.
    pub(crate) fn draft_width(&self) -> usize {
        if self.review_list.is_open() {
            return self
                .column_width()
                .saturating_sub(crate::app::threads::list::BODY_INDENT)
                .max(1);
        }
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
        if self.review_list.is_open() {
            return self.review_draft_cursor_cell();
        }
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
    fn submit_annotation(
        &mut self,
        evidence: &AnnotationEvidence,
        comment: String,
        submission: UserSubmit,
    ) -> Result<SubmitResult, String> {
        if !self.annotation_projection_matches_selection() {
            return Err("comparison projection changed; keeping the annotation draft".to_owned());
        }
        let path = &evidence.path;
        let range = evidence.range;
        // The thread belongs to the work it was written against (ADR 0024).
        let draft = match range {
            Some(range) => Draft::new(Author::User, path, range, comment),
            None => Draft::on_file(Author::User, path, comment),
        }
        .at_source(evidence.provenance.version().clone(), evidence.side)
        .with_provenance(evidence.provenance.clone());
        let where_at = range.map_or_else(|| "the file".to_owned(), |range| format!("L{range}"));
        let Some(store) = self.store_mut() else {
            return Err(self.thread_store_unavailable());
        };
        let result = store.annotate_user(draft, &evidence.text, now(), submission);
        self.refresh_after_thread_store_change();
        match result {
            Ok(id) => {
                self.trigger_landing(None);
                tracing::info!(%id, path = %path.display(), %where_at, "thread started");
                self.view_mut().clear_selection();
                self.notice(format!("commented on {where_at}"));
                Ok(SubmitResult::Applied)
            }

            Err(error) => Err(format!("cannot save comment: {error}")),
        }
    }

    fn submit_reply(
        &mut self,
        id: &ThreadId,
        body: String,
        submission: UserSubmit,
        reopen: bool,
    ) -> Result<SubmitResult, String> {
        let Some(store) = self.store_mut() else {
            return Err(self.thread_store_unavailable());
        };
        let result = if reopen {
            store
                .reply_user(id, now(), body, submission)
                .map(|()| UserWriteOutcome::Applied)
        } else {
            store.reply_user_if_unresolved(id, now(), body, submission)
        };
        self.refresh_after_thread_store_change();
        match result {
            Ok(UserWriteOutcome::Applied) => {
                tracing::info!(%id, "reply added");
                // The reply becomes the highlighted message, under the
                // draft's rows it replaces (ADR 0049).
                let newest = self.newest_message(id);
                self.goto_message(id.clone(), newest);
                Ok(SubmitResult::Applied)
            }
            Ok(UserWriteOutcome::ReopenRequired) => Ok(SubmitResult::ReopenRequired),
            Err(error) => Err(format!("cannot save reply: {error}")),
        }
    }

    fn submit_message_edit(
        &mut self,
        id: &ThreadId,
        target: MessageTarget,
        body: String,
        submission: UserSubmit,
        reopen: bool,
    ) -> Result<SubmitResult, String> {
        let Some(store) = self.store_mut() else {
            return Err(self.thread_store_unavailable());
        };
        let result = if reopen {
            store
                .edit_user(id, target, body, now(), submission)
                .map(|()| UserWriteOutcome::Applied)
        } else {
            store.edit_user_if_unresolved(id, target, body, now(), submission)
        };
        self.refresh_after_thread_store_change();
        match result {
            Ok(UserWriteOutcome::Applied) => {
                tracing::info!(%id, ?target, "thread message edited");
                self.follow_cursor_message();
                self.notice("message edited");
                Ok(SubmitResult::Applied)
            }
            Ok(UserWriteOutcome::ReopenRequired) => Ok(SubmitResult::ReopenRequired),
            Err(error) => Err(format!("cannot edit message: {error}")),
        }
    }
}

impl App {
    fn annotation_evidence(&self, range: Option<LineRange>) -> Option<AnnotationEvidence> {
        let index = self.current?;
        let path = self.docs[index].relative.clone();
        let displayed_text = self.docs[index].view.text();
        let (text, side) = self.annotation_source(range, displayed_text);
        let provenance = self.user_provenance(&path, range, &text, side);
        Some(AnnotationEvidence {
            path,
            range,
            text,
            side,
            provenance,
        })
    }

    fn annotation_source(
        &self,
        range: Option<LineRange>,
        displayed_text: &str,
    ) -> (String, OriginSide) {
        let Some(range) = range else {
            return (displayed_text.to_owned(), self.displayed_source_side());
        };
        let Some((side, source_range)) = displayed_diff_side_and_range(self.view()) else {
            return (displayed_text.to_owned(), self.displayed_source_side());
        };
        if side != OriginSide::Base {
            return (displayed_text.to_owned(), side);
        }
        let Some(base) = self.view().comparison_base_text() else {
            return (displayed_text.to_owned(), side);
        };
        if source_range != range {
            return (displayed_text.to_owned(), OriginSide::Base);
        }
        (base.to_owned(), side)
    }

    fn user_provenance(
        &self,
        path: &std::path::Path,
        _range: Option<LineRange>,
        text: &str,
        side: OriginSide,
    ) -> Provenance {
        let effective = (self.diff_mode() != fathomable_core::config::DiffMode::Off)
            .then(|| self.comparison())
            .flatten();
        let endpoint = if side == OriginSide::Base {
            effective.map_or_else(
                || self.comparison.base(),
                fathomable_core::diff::Comparison::base,
            )
        } else {
            effective.map_or_else(
                || self.comparison.target(),
                fathomable_core::diff::Comparison::target,
            )
        };
        let version = endpoint_version(self, endpoint);
        let mut provenance = Provenance::new(version, side);
        if self.diff_mode() != fathomable_core::config::DiffMode::Off {
            provenance = provenance.with_comparison(comparison_facts(self));
        }
        match endpoint {
            fathomable_core::workspace::ComparisonEndpoint::WorkingTree => {
                let state = self.status.get(path).map_or_else(
                    || {
                        if self.workspace.is_git() {
                            WorkingTreeState::Clean
                        } else {
                            WorkingTreeState::Added
                        }
                    },
                    |entry| match entry.state() {
                        fathomable_core::status::State::Modified => WorkingTreeState::Modified,
                        fathomable_core::status::State::Deleted => WorkingTreeState::Deleted,
                        fathomable_core::status::State::Added
                        | fathomable_core::status::State::Untracked => WorkingTreeState::Added,
                    },
                );
                provenance = provenance.with_working_tree(WorkingTreeFacts::new(
                    self.workspace.head_commit(),
                    state,
                    Some(ContentIdentity::from_text(text)),
                    self.workspace.identity(),
                    FullFileDigest::from_bytes(text.as_bytes()),
                ));
            }
            fathomable_core::workspace::ComparisonEndpoint::Index => {
                let state = self
                    .status
                    .get(path)
                    .filter(|entry| entry.staged_state().is_some())
                    .map_or(IndexState::Unchanged, |_| IndexState::Staged);
                provenance = provenance.with_index(IndexFacts::new(
                    self.workspace.head_commit(),
                    state,
                    Some(ContentIdentity::from_text(text)),
                    self.workspace.identity(),
                    FullFileDigest::from_bytes(text.as_bytes()),
                ));
            }
            fathomable_core::workspace::ComparisonEndpoint::ReviewPoint(id) => {
                let facts = self
                    .review_points
                    .as_ref()
                    .and_then(|store| store.get(id))
                    .map(|point| {
                        ReviewPointFacts::new(
                            point.id(),
                            point.head().map(ToString::to_string),
                            Some(ContentIdentity::from_text(text)),
                            point.checkout_identity(),
                            FullFileDigest::from_bytes(text.as_bytes()),
                        )
                    });
                if let Some(facts) = facts {
                    provenance = provenance.with_review_point(facts);
                }
            }
            fathomable_core::workspace::ComparisonEndpoint::Commit(_)
            | fathomable_core::workspace::ComparisonEndpoint::EmptyTree => {}
        }
        provenance
    }

    fn displayed_source_side(&self) -> OriginSide {
        if self.diff_mode() == fathomable_core::config::DiffMode::Off {
            return OriginSide::Target;
        }
        let Some(comparison) = self.comparison() else {
            return OriginSide::Target;
        };
        let path = self.current_path();
        let target_absent = comparison
            .changes()
            .iter()
            .find(|change| change.path() == path)
            .is_some_and(|change| {
                matches!(change.target(), fathomable_core::diff::PathState::Absent)
            });
        if target_absent {
            OriginSide::Base
        } else {
            OriginSide::Target
        }
    }
}

fn comparison_facts(app: &App) -> ComparisonFacts {
    let base = endpoint_version(app, app.comparison.base());
    let target = endpoint_version(app, app.comparison.target());
    if matches!(
        &base,
        OriginVersion::WorkingTree { .. } | OriginVersion::Index { .. }
    ) || matches!(
        &target,
        OriginVersion::WorkingTree { .. } | OriginVersion::Index { .. }
    ) {
        ComparisonFacts::at_checkout(base, target, app.workspace.identity())
    } else {
        ComparisonFacts::new(base, target)
    }
}

fn endpoint_version(
    app: &App,
    endpoint: &fathomable_core::workspace::ComparisonEndpoint,
) -> OriginVersion {
    match endpoint {
        fathomable_core::workspace::ComparisonEndpoint::EmptyTree => OriginVersion::EmptyTree,
        fathomable_core::workspace::ComparisonEndpoint::Commit(id) => {
            OriginVersion::commit(id.to_string())
        }
        fathomable_core::workspace::ComparisonEndpoint::ReviewPoint(id) => {
            let base = app
                .review_points
                .as_ref()
                .and_then(|store| store.get(id))
                .and_then(|point| point.head())
                .map(ToString::to_string);
            OriginVersion::review_point(id.clone(), base)
        }
        fathomable_core::workspace::ComparisonEndpoint::Index => {
            OriginVersion::index(app.workspace.head_commit())
        }
        fathomable_core::workspace::ComparisonEndpoint::WorkingTree => {
            OriginVersion::working_tree(app.workspace.head_commit())
        }
    }
}

fn mixed_diff_selection(
    view: &crate::app::view::View,
    selection: crate::app::view::Selection,
) -> bool {
    let (start, end) = selection.ordered();
    diff_rows(view, start.row..=end.row)
        .is_some_and(|rows| rows.base.is_some() && rows.target.is_some())
}

fn cursor_on_removed_diff_line(view: &crate::app::view::View) -> bool {
    view.layout()
        .lines()
        .get(view.cursor().row)
        .is_some_and(|line| line.diff_old_line().is_some() && line.diff_new_line().is_none())
}

fn displayed_diff_range(view: &crate::app::view::View) -> Option<LineRange> {
    displayed_diff_side_and_range(view).map(|(_, range)| range)
}

fn displayed_diff_side_and_range(view: &crate::app::view::View) -> Option<(OriginSide, LineRange)> {
    let rows = view.selection().map_or_else(
        || view.cursor().row..=view.cursor().row,
        |selection| {
            let (start, end) = selection.ordered();
            start.row..=end.row
        },
    );
    let rows = diff_rows(view, rows)?;
    match (rows.base, rows.target) {
        (Some(range), None) => Some((OriginSide::Base, range)),
        (None, Some(range)) => Some((OriginSide::Target, range)),
        _ => None,
    }
}

#[derive(Debug, Default)]
struct DiffRows {
    base: Option<LineRange>,
    target: Option<LineRange>,
}

fn diff_rows(
    view: &crate::app::view::View,
    rows: std::ops::RangeInclusive<usize>,
) -> Option<DiffRows> {
    let mut found = DiffRows::default();
    for row in rows {
        let line = view.layout().lines().get(row)?;
        match (line.diff_old_line(), line.diff_new_line()) {
            (Some(old), None) => extend_range(&mut found.base, old),
            (_, Some(new)) => extend_range(&mut found.target, new),
            (None, None) if view.projects_normal_deletion() => {
                if let Some(source) = line.source() {
                    let index = view.layout().index();
                    let start = index.line_of(source.start);
                    let end = index.line_of(source.end.max(source.start + 1) - 1);
                    extend_range(&mut found.target, start);
                    extend_range(&mut found.target, end);
                }
            }
            (None, None) => {}
        }
    }
    (found.base.is_some() || found.target.is_some()).then_some(found)
}

fn extend_range(range: &mut Option<LineRange>, line: usize) {
    *range = Some(match *range {
        Some(current) => LineRange::new(current.start().min(line), current.end().max(line)),
        None => LineRange::new(line, line),
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SubmitResult {
    Applied,
    ReopenRequired,
}
