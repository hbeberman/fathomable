// @okf-doc: /decisions/0013-annotation-storage-and-ux.md
//! Annotations on top of the view: gutter marks, the comment box, and the
//! thread panel.
//!
//! [`App`] keeps the [`Store`] for the workspace; every open document
//! carries the [`Mark`]s of its threads, re-located whenever the text
//! changes. The comment box ([`Compose`]) starts a thread or replies to
//! one; the [`ThreadPanel`] reads a thread and resolves it. All of it is
//! plain state so ADR 0013 behaviour is tested without a terminal.

use std::time::{SystemTime, UNIX_EPOCH};

use fathomable_core::annotations::{
    Author, Draft, LineRange, Placement, Reply, Status, Store, Thread, ThreadId,
};
use fathomable_core::editor::{Buffer, Cell, Edit};
use fathomable_core::reanchor::{Mapping, map_range};

use super::ui::SNIPPET_ROWS;
use super::{App, Focus, Popup};

/// How a thread should be coloured in the gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MarkKind {
    Resolved,
    AutoResolved,
    Open,
    /// The lines under the comment were rewritten since the user last
    /// answered (ADR 0019).
    Edited,
    /// The annotated lines are gone; shown at the last known range.
    Detached,
}

impl MarkKind {
    fn of(thread: &Thread, placement: Placement) -> Self {
        if placement.is_detached() {
            return Self::Detached;
        }
        if placement.is_edited() {
            return Self::Edited;
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
    kind: MarkKind,
}

impl Mark {
    pub fn id(&self) -> &ThreadId {
        &self.id
    }

    pub fn range(&self) -> LineRange {
        self.placement.range()
    }

    pub fn kind(&self) -> MarkKind {
        self.kind
    }
}

/// What the comment box will produce on submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeTarget {
    /// A new thread on these source lines.
    New(LineRange),
    /// A reply to an existing thread.
    Reply(ThreadId),
}

/// The multi-line comment box (ADR 0005).
#[derive(Debug)]
pub struct Compose {
    target: ComposeTarget,
    buffer: Buffer,
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

/// The open thread panel: the threads under the cursor and which is shown.
#[derive(Debug)]
pub struct ThreadPanel {
    ids: Vec<ThreadId>,
    index: usize,
    scroll: usize,
}

impl ThreadPanel {
    pub fn id(&self) -> &ThreadId {
        &self.ids[self.index.min(self.ids.len() - 1)]
    }

    /// `(current, total)`, 1-based, for the panel header.
    pub fn position(&self) -> (usize, usize) {
        (self.index + 1, self.ids.len())
    }

    pub fn scroll(&self) -> usize {
        self.scroll
    }
}

fn overlaps(a: LineRange, b: LineRange) -> bool {
    a.start() <= b.end() && b.start() <= a.end()
}

/// Seconds since the Unix epoch.
pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

impl App {
    /// The store, or a status-line notice explaining why there is none.
    pub(super) fn store_mut(&mut self) -> Option<&mut Store> {
        if self.store.is_none() {
            self.notice("annotations unavailable; see the log");
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

    /// The most urgent mark overlapping `lines` (a rendered row can carry
    /// several source lines).
    pub fn mark_in(&self, lines: LineRange) -> Option<MarkKind> {
        self.marks()
            .iter()
            .filter(|mark| overlaps(mark.range(), lines))
            .map(Mark::kind)
            .max()
    }

    /// `(open, total)` threads on the current document.
    pub fn thread_counts(&self) -> (usize, usize) {
        let open = self
            .marks()
            .iter()
            .filter(|mark| matches!(mark.kind(), MarkKind::Open | MarkKind::Detached))
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
        let text = doc.document.text().to_owned();
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
        let text = doc.document.text();
        doc.marks = store
            .for_path(&doc.relative)
            .filter(|thread| self.scope.includes(thread))
            .map(|thread| {
                let placement = thread.locate(text);
                Mark {
                    id: thread.id().clone(),
                    placement,
                    kind: MarkKind::of(thread, placement),
                }
            })
            .collect();
        let detached = doc
            .marks
            .iter()
            .filter(|m| m.kind() == MarkKind::Detached)
            .count();
        tracing::debug!(path = %doc.relative.display(), marks = doc.marks.len(), detached, "marks refreshed");
    }

    /// Re-locate every loaded document's threads, after a change that
    /// may touch files other than the current one (ADR 0025).
    pub(super) fn refresh_all_marks(&mut self) {
        for index in 0..self.docs.len() {
            self.refresh_marks(index);
        }
    }

    /// Threads whose range touches the cursor's rendered row.
    pub fn threads_at_cursor(&self) -> Vec<ThreadId> {
        let view = self.view();
        let lines = view.source_lines_of_row(view.cursor().row).or_else(|| {
            view.cursor_source_line()
                .map(|line| LineRange::new(line, line))
        });
        let Some(lines) = lines else {
            return Vec::new();
        };
        self.marks()
            .iter()
            .filter(|mark| overlaps(mark.range(), lines))
            .map(|mark| mark.id().clone())
            .collect()
    }

    // ----- comment box -----

    /// `c`: open the comment box on the selection, or the cursor line.
    pub fn start_comment(&mut self) {
        if self.current.is_none() {
            self.notice("open a file to annotate it");
            return;
        }
        if self.store.is_none() {
            self.store_mut();
            return;
        }
        let view = self.view();
        let range = view.selected_lines().or_else(|| {
            view.cursor_source_line()
                .map(|line| LineRange::new(line, line))
        });
        let Some(range) = range else {
            self.notice("nothing to annotate here");
            return;
        };
        self.open_compose(ComposeTarget::New(range));
    }

    pub(super) fn open_compose(&mut self, target: ComposeTarget) {
        self.popup = Some(Popup::Compose(Compose {
            target,
            buffer: Buffer::new(),
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
    /// non-empty draft asks for a second Esc first.
    pub fn compose_cancel(&mut self) {
        let Some(Popup::Compose(compose)) = self.popup.as_mut() else {
            return;
        };
        if !compose.confirm_discard && !compose.buffer.text().trim().is_empty() {
            compose.confirm_discard = true;
            self.notice("Esc again to discard the comment");
            return;
        }
        self.popup = None;
        self.refocus_after_compose();
        self.notice("comment cancelled");
    }

    /// Ctrl-Enter / Alt-Enter: write the comment to the store.
    pub fn compose_submit(&mut self) {
        let Some(Popup::Compose(compose)) = self.popup.take() else {
            return;
        };
        let text = compose.buffer.text().trim().to_owned();
        if text.is_empty() {
            self.notice("empty comment discarded");
            return;
        }
        match compose.target {
            ComposeTarget::New(range) => self.submit_annotation(range, text),
            ComposeTarget::Reply(id) => self.submit_reply(&id, text),
        }
        self.refocus_after_compose();
    }

    /// The box closed: the keys go back to the pane or the list it was
    /// opened from.
    fn refocus_after_compose(&mut self) {
        if self.thread.is_some() {
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
        let text = self.docs[index].document.text().to_owned();
        // The thread belongs to the work it was written against (ADR 0024).
        let draft = Draft::new(&path, range, comment).at_commit(self.workspace.head_commit());
        let Some(store) = self.store_mut() else {
            return;
        };
        match store.annotate(draft, &text, now()) {
            Ok(id) => {
                tracing::info!(%id, path = %path.display(), %range, "thread started");
                self.refresh_scope();
                self.refresh_marks(index);
                // Commenting on lines means they were read (ADR 0020).
                self.mark_seen(index);
                self.view_mut().clear_selection();
                self.notice(format!("annotated L{range}"));
            }
            Err(error) => self.notice(format!("cannot save annotation: {error}")),
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
                if !self.list.is_open() {
                    self.open_thread(id.clone());
                }
            }
            Err(error) => self.notice(format!("cannot save reply: {error}")),
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
    ) -> Result<(), String> {
        let store = self
            .store
            .as_mut()
            .ok_or("annotations unavailable; see the log")?;
        if store.thread(id).is_none() {
            return Err(format!("unknown thread {id}"));
        }
        let when = now();
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
        for index in 0..self.docs.len() {
            self.refresh_marks(index);
        }
        if self.thread.as_ref().is_some_and(|panel| panel.id() == id) {
            self.open_thread(id.clone());
        }
        Ok(())
    }

    // ----- thread pane -----

    /// `Space a`: show the thread(s) under the cursor.
    pub fn open_thread_at_cursor(&mut self) {
        let ids = self.threads_at_cursor();
        if ids.is_empty() {
            self.notice("no thread on this line");
            return;
        }
        self.show_panel(ThreadPanel {
            ids,
            index: 0,
            scroll: 0,
        });
    }

    /// Show one thread; the pane lists only it.
    pub fn open_thread(&mut self, id: ThreadId) {
        let scroll = self
            .thread
            .as_ref()
            .filter(|panel| panel.id() == &id)
            .map_or(0, ThreadPanel::scroll);
        self.show_panel(ThreadPanel {
            ids: vec![id],
            index: 0,
            scroll,
        });
    }

    /// The pane opens along the bottom of the text and takes the keys
    /// unless the comment box is up.
    fn show_panel(&mut self, panel: ThreadPanel) {
        self.thread = Some(panel);
        if self.popup.is_none() {
            self.focus = Focus::Thread;
        }
        self.relayout();
    }

    /// Esc in the pane: close it and hand the keys back to the text.
    pub fn close_thread(&mut self) {
        self.thread = None;
        if self.focus == Focus::Thread {
            self.focus = Focus::View;
        }
        self.relayout();
    }

    fn panel_mut(&mut self) -> Option<&mut ThreadPanel> {
        self.thread.as_mut()
    }

    /// `n` / `p`: another thread on the same line.
    pub fn thread_step(&mut self, delta: isize) {
        if let Some(panel) = self.panel_mut() {
            let len = panel.ids.len();
            let step = delta.rem_euclid(len.cast_signed()).cast_unsigned();
            panel.index = (panel.index + step) % len;
            panel.scroll = 0;
        }
    }

    /// `j` / `k`: scroll the panel text.
    pub fn thread_scroll(&mut self, delta: isize) {
        let Some(panel) = self.panel_mut() else {
            return;
        };
        let id = panel.id().clone();
        let scroll = panel.scroll.saturating_add_signed(delta);
        // The rendered height depends on wrapping, so the panel clamps to the
        // unwrapped line count: `j` past the end never runs away, and the
        // drawing code trims the remainder.
        let limit = self.thread(&id).map_or(0, |thread| {
            let replies: usize = thread
                .replies()
                .iter()
                .map(|reply| reply.body().lines().count() + 2)
                .sum();
            SNIPPET_ROWS + 2 + thread.comment().lines().count() + replies
        });
        if let Some(panel) = self.panel_mut() {
            panel.scroll = scroll.min(limit);
        }
    }

    /// `r`: reply to the shown thread through the comment box; the pane
    /// stays readable above it.
    pub fn thread_reply(&mut self) {
        if let Some(id) = self.thread.as_ref().map(|panel| panel.id().clone()) {
            self.open_compose(ComposeTarget::Reply(id));
        }
    }

    /// `x`: resolve the shown thread, or reopen it when already resolved.
    pub fn thread_toggle_resolved(&mut self) {
        if let Some(id) = self.panel_mut().map(|panel| panel.id().clone()) {
            self.toggle_resolved(&id);
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

    // ----- navigation -----

    /// `]c`: the next mark below the cursor, wrapping to the top.
    pub fn next_annotation(&mut self) {
        self.jump_annotation(1);
    }

    /// `[c`: the previous mark above the cursor, wrapping to the bottom.
    pub fn prev_annotation(&mut self) {
        self.jump_annotation(-1);
    }

    fn jump_annotation(&mut self, direction: isize) {
        let mut starts: Vec<usize> = self.marks().iter().map(|m| m.range().start()).collect();
        starts.sort_unstable();
        starts.dedup();
        if starts.is_empty() {
            self.notice("no threads in this file");
            return;
        }
        let line = self.view().cursor_source_line().unwrap_or(0);
        let (target, wrapped) = if direction > 0 {
            starts
                .iter()
                .find(|&&start| start > line)
                .map_or((starts[0], true), |&start| (start, false))
        } else {
            starts
                .iter()
                .rev()
                .find(|&&start| start < line)
                .map_or((starts[starts.len() - 1], true), |&start| (start, false))
        };
        if wrapped {
            self.notice(if direction > 0 {
                "wrapped to first thread"
            } else {
                "wrapped to last thread"
            });
        }
        self.view_mut().goto_source_line(target);
    }

    /// The list's Enter (ADR 0025): jump to `id` and open the pane on it.
    pub(super) fn show_thread(&mut self, id: ThreadId) {
        if let Some(line) = self
            .marks()
            .iter()
            .find(|mark| *mark.id() == id)
            .map(|mark| mark.range().start())
        {
            self.view_mut().goto_source_line(line);
        }
        self.open_thread(id);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::annotations::{LineRange, Status, Store, Thread};
    use fathomable_core::workspace::Workspace;

    use fathomable_core::annotations::Author;
    use fathomable_core::session::{Request, Response};

    use anyhow::Context as _;

    use crate::app::Focus;

    use fathomable_core::editor::{Cursor, Edit, Motion};

    use super::{ComposeTarget, MarkKind};
    use crate::app::thread_list::Row;
    use crate::app::{App, Border, Options, Popup};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> std::io::Result<Self> {
            let dir = std::env::temp_dir()
                .join(format!("fathomable-threads-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("ws"))?;
            fs::write(
                dir.join("ws/README.md"),
                "# Readme\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
            )?;
            Ok(Self(dir))
        }

        fn app(&self) -> anyhow::Result<App> {
            let workspace = Workspace::discover(self.0.join("ws"))?;
            let store = Store::open(self.0.join("state/threads.jsonl"))?;
            let options = Options {
                store: Some(store),
                ..Options::for_test(self.0.join("ws"))
            };
            let mut app = App::new(workspace, 100, 30, options);
            app.open(Path::new("README.md"));
            Ok(app)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
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

    #[test]
    fn selection_becomes_a_thread_and_survives_reload() -> anyhow::Result<()> {
        let dir = TempDir::new("annotate")?;
        let mut app = dir.app()?;
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
        assert_eq!(app.message(), Some("annotated L3-5"));
        assert_eq!(app.thread_counts(), (1, 1));
        assert_eq!(app.mark_in(LineRange::new(4, 4)), Some(MarkKind::Open));
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
        assert_eq!(app.mark_in(LineRange::new(6, 6)), Some(MarkKind::Edited));
        let reopened = Store::open(dir.0.join("state/threads.jsonl"))?;
        assert!(reopened.threads()[0].edited().is_some());
        assert_eq!(reopened.threads()[0].range(), LineRange::new(5, 7));

        // The user's reply acknowledges the edit.
        app.view_mut().move_down(3);
        app.open_thread_at_cursor();
        app.thread_reply();
        type_in(&mut app, "still fine");
        app.compose_submit();
        assert_eq!(app.mark_in(LineRange::new(6, 6)), Some(MarkKind::Open));

        // Rewrite everything: the thread detaches at its last known range.
        fs::write(dir.0.join("ws/README.md"), "# Readme\n\ngone\n")?;
        app.on_changes(vec![dir.0.join("ws/README.md")]);
        assert_eq!(app.mark_in(LineRange::new(5, 5)), Some(MarkKind::Detached));
        Ok(())
    }

    #[test]
    fn thread_panel_renders_header_snippet_badge_and_overflow() -> anyhow::Result<()> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let dir = TempDir::new("render")?;
        let mut app = dir.app()?;
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
            },
            long,
            true,
        )
        .map_err(anyhow::Error::msg)?;
        app.open_thread(id);

        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::ui::Theme::from_core(&core);
        let render = |app: &App| -> anyhow::Result<Vec<String>> {
            let mut terminal = Terminal::new(TestBackend::new(80, 36))?;
            terminal.draw(|frame| crate::app::ui::draw(frame, app, &theme))?;
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
        let panel = render(&app)?;
        let screen = panel.join("\n");
        assert!(
            panel[0].contains("thread  L3-5  auto-resolved"),
            "header carries range and status:\n{screen}"
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
        let dir = TempDir::new("panel")?;
        let mut app = dir.app()?;
        app.open_thread_at_cursor();
        assert_eq!(app.message(), Some("no thread on this line"));
        app.start_comment();
        type_in(&mut app, "first");
        app.compose_submit();
        assert_eq!(app.mark_in(LineRange::new(1, 1)), Some(MarkKind::Open));

        app.open_thread_at_cursor();
        let Some(panel) = app.thread_panel() else {
            anyhow::bail!("panel did not open");
        };
        assert_eq!(panel.position(), (1, 1));
        assert_eq!(app.focus(), Focus::Thread);
        let id = panel.id().clone();
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
        assert_eq!(app.mark_in(LineRange::new(1, 1)), Some(MarkKind::Resolved));
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
    fn mouse_targets_the_pane_under_the_pointer() -> anyhow::Result<()> {
        use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

        use super::ThreadPanel;
        use crate::app::{Border, keys};

        let mouse = |kind, column: usize, row: usize| MouseEvent {
            kind,
            column: u16::try_from(column).unwrap_or(u16::MAX),
            row: u16::try_from(row).unwrap_or(u16::MAX),
            modifiers: KeyModifiers::NONE,
        };
        let down = MouseEventKind::Down(MouseButton::Left);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);

        let dir = TempDir::new("mouse")?;
        let mut app = dir.app()?;
        app.start_comment();
        type_in(&mut app, "first");
        app.compose_submit();
        app.open_thread_at_cursor();
        assert_eq!(app.focus(), Focus::Thread);
        let rows = app.pane_rows();
        let top = rows - app.thread_rows();
        assert_eq!(app.text_rows(), top, "the pane takes rows from the text");

        // The wheel scrolls the pane under the pointer, focus aside.
        keys::handle_mouse(&mut app, mouse(MouseEventKind::ScrollDown, 20, top + 2));
        assert_eq!(app.thread_panel().map(ThreadPanel::scroll), Some(3));
        assert_eq!(app.view().scroll(), 0);
        keys::handle_mouse(&mut app, mouse(down, 20, 0));
        assert_eq!(app.focus(), Focus::View, "a click on the text focuses it");
        assert!(app.thread_panel().is_some(), "the pane stays open");
        keys::handle_mouse(&mut app, mouse(MouseEventKind::ScrollUp, 20, top + 2));
        assert_eq!(app.thread_panel().map(ThreadPanel::scroll), Some(0));
        keys::handle_mouse(&mut app, mouse(down, 20, top + 2));
        assert_eq!(app.focus(), Focus::Thread, "a click on the pane focuses it");

        // Dragging the rule resizes the pane.
        keys::handle_mouse(&mut app, mouse(down, 20, top));
        assert_eq!(app.dragging(), Some(Border::Thread));
        keys::handle_mouse(&mut app, mouse(drag, 20, top - 4));
        assert_eq!(app.thread_rows(), rows - top + 4);
        assert_eq!(app.text_rows(), top - 4);
        keys::handle_mouse(&mut app, mouse(up, 20, top - 4));
        assert_eq!(app.dragging(), None);
        assert_eq!(app.view().selection(), None, "a border drag never selects");

        // Dragging the tree's divider resizes the tree.
        app.toggle_sidebar_focus();
        let width = app.sidebar_width();
        keys::handle_mouse(&mut app, mouse(down, width - 1, 3));
        assert_eq!(app.dragging(), Some(Border::Sidebar));
        keys::handle_mouse(&mut app, mouse(drag, 44, 3));
        assert_eq!(app.sidebar_width(), 45);
        keys::handle_mouse(&mut app, mouse(drag, 2, 3));
        assert_eq!(app.sidebar_width(), 8, "no narrower than the minimum");
        keys::handle_mouse(&mut app, mouse(up, 2, 3));
        keys::handle_mouse(&mut app, mouse(down, 3, 0));
        assert_eq!(
            app.focus(),
            Focus::Sidebar,
            "the header row focuses the tree"
        );

        // The comment box keeps the keys but lets the mouse through.
        keys::handle_mouse(&mut app, mouse(down, 20, 0));
        app.thread_reply();
        assert!(matches!(app.popup(), Some(Popup::Compose(_))));
        let top = rows - app.thread_rows();
        keys::handle_mouse(&mut app, mouse(MouseEventKind::ScrollDown, 20, top + 2));
        assert_eq!(app.thread_panel().map(ThreadPanel::scroll), Some(3));
        keys::handle_mouse(&mut app, mouse(down, 20, 0));
        assert!(
            matches!(app.popup(), Some(Popup::Compose(_))),
            "a click away leaves the box open"
        );
        Ok(())
    }

    #[test]
    fn annotation_jumps_wrap_and_picker_lists_threads() -> anyhow::Result<()> {
        let dir = TempDir::new("jumps")?;
        let mut app = dir.app()?;
        app.next_annotation();
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
        app.next_annotation();
        assert_eq!(app.view().cursor_source_line(), bottom);
        app.next_annotation();
        assert_eq!(app.view().cursor_source_line(), Some(1));
        assert_eq!(app.message(), Some("wrapped to first thread"));
        app.prev_annotation();
        assert_eq!(app.view().cursor_source_line(), bottom);
        app.start_comment();
        app.compose_submit();
        assert_eq!(app.message(), Some("empty comment discarded"));
        Ok(())
    }

    #[test]
    fn the_thread_list_shows_the_work_and_acts_in_place() -> anyhow::Result<()> {
        let dir = TempDir::new("list")?;
        let mut app = dir.app()?;
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
        app.thread_list_move(1);
        app.thread_list_open_entry();
        assert!(!app.thread_list().is_open());
        assert_eq!(app.focus(), Focus::Thread);
        assert!(app.thread_panel().is_some());
        assert_eq!(app.view().cursor_source_line(), bottom);

        // `x` moves an entry to the resolved section; `f` narrows to the
        // file; `Z` folds the resolved section; reopening keeps the entry.
        app.open_thread_list();
        app.thread_list_toggle_resolved();
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
        app.thread_list_reply();
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
        let dir = TempDir::new("socket")?;
        fs::write(dir.0.join("ws/other.md"), "# Other\n\nline\n")?;
        let mut app = dir.app()?;
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
        };
        let reply = app.handle_request(Request::ThreadReply {
            thread: id.clone(),
            author: author.clone(),
            body: "fixed".to_owned(),
            resolve: true,
        });
        assert_eq!(reply, Response::Done);
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
        let dir = TempDir::new("editor")?;
        let mut app = dir.app()?;
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
        let dir = TempDir::new("discard")?;
        let mut app = dir.app()?;
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
    fn a_long_comment_wraps_and_scrolls_to_the_cursor() -> anyhow::Result<()> {
        let dir = TempDir::new("wrap")?;
        let mut app = dir.app()?;
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

        let dir = TempDir::new("sizes")?;
        let mut app = dir.app()?;
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
            },
            (1..=12)
                .map(|n| format!("line {n}"))
                .collect::<Vec<_>>()
                .join("\n"),
            true,
        )
        .map_err(anyhow::Error::msg)?;

        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::ui::Theme::from_core(&core);
        let draw = |app: &mut App, state: &str| -> anyhow::Result<()> {
            for width in [1u16, 2, 4, 8, 12, 20, 40, 80] {
                for height in 1..=6u16 {
                    app.resize(usize::from(width), usize::from(height));
                    let mut terminal = Terminal::new(TestBackend::new(width, height))?;
                    terminal
                        .draw(|frame| crate::app::ui::draw(frame, app, &theme))
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
        app.thread_list_reply();
        type_in(&mut app, "a reply from the list");
        draw(&mut app, "compose over list")?;
        Ok(())
    }
}
