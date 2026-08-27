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

use super::ui::SNIPPET_ROWS;
use super::{App, PickerKind, PickerState, Popup};

/// How a thread should be coloured in the gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MarkKind {
    Resolved,
    AutoResolved,
    Open,
    /// The annotated lines are gone; shown at the last known range.
    Detached,
}

impl MarkKind {
    fn of(thread: &Thread, placement: Placement) -> Self {
        if placement.is_detached() {
            return Self::Detached;
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
    text: String,
    /// The thread panel a reply was started from; it stays on screen
    /// above the box and comes back on cancel.
    panel: Option<ThreadPanel>,
}

impl Compose {
    pub fn target(&self) -> &ComposeTarget {
        &self.target
    }

    /// The thread being replied to, still readable while typing.
    pub fn panel(&self) -> Option<&ThreadPanel> {
        self.panel.as_ref()
    }

    pub fn text(&self) -> &str {
        &self.text
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
pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// One line describing a thread for the picker: range, status, comment.
fn describe(thread: &Thread, placement: Placement) -> String {
    let status = match (placement.is_detached(), thread.status()) {
        (true, _) => "detached",
        (false, Status::Open) => "open",
        (false, Status::Resolved) => "resolved",
        (false, Status::AutoResolved) => "auto-resolved",
    };
    let first = thread.comment().lines().next().unwrap_or_default();
    format!("L{:<7} {status:<13} {first}", placement.range().to_string())
}

impl App {
    /// The store, or a status-line notice explaining why there is none.
    fn store_mut(&mut self) -> Option<&mut Store> {
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

    fn refresh_current_marks(&mut self) {
        if let Some(index) = self.current {
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
        self.open_compose(ComposeTarget::New(range), None);
    }

    fn open_compose(&mut self, target: ComposeTarget, panel: Option<ThreadPanel>) {
        self.popup = Some(Popup::Compose(Compose {
            target,
            text: String::new(),
            panel,
        }));
    }

    fn compose_mut(&mut self) -> Option<&mut Compose> {
        match self.popup.as_mut() {
            Some(Popup::Compose(compose)) => Some(compose),
            _ => None,
        }
    }

    pub fn compose_char(&mut self, ch: char) {
        if let Some(compose) = self.compose_mut() {
            compose.text.push(ch);
        }
    }

    /// Enter: a new line in the comment.
    pub fn compose_newline(&mut self) {
        self.compose_char('\n');
    }

    pub fn compose_backspace(&mut self) {
        if let Some(compose) = self.compose_mut() {
            compose.text.pop();
        }
    }

    /// Up / Down while replying: scroll the thread shown above the box.
    pub fn compose_scroll(&mut self, delta: isize) {
        if let Some(panel) = self.compose_mut().and_then(|c| c.panel.as_mut()) {
            panel.scroll = panel.scroll.saturating_add_signed(delta);
        }
    }

    /// Esc: drop the comment box; a reply returns to its thread panel.
    pub fn compose_cancel(&mut self) {
        if let Some(Popup::Compose(compose)) = self.popup.take() {
            self.popup = compose.panel.map(Popup::Thread);
            self.notice("comment cancelled");
        }
    }

    /// Ctrl-Enter / Alt-Enter: write the comment to the store.
    pub fn compose_submit(&mut self) {
        let Some(Popup::Compose(compose)) = self.popup.take() else {
            return;
        };
        let text = compose.text.trim().to_owned();
        if text.is_empty() {
            self.notice("empty comment discarded");
            return;
        }
        match compose.target {
            ComposeTarget::New(range) => self.submit_annotation(range, text),
            ComposeTarget::Reply(id) => self.submit_reply(&id, text),
        }
    }

    fn submit_annotation(&mut self, range: LineRange, comment: String) {
        let Some(index) = self.current else {
            return;
        };
        let path = self.docs[index].relative.clone();
        let text = self.docs[index].document.text().to_owned();
        let draft = Draft::new(&path, range, comment);
        let Some(store) = self.store_mut() else {
            return;
        };
        match store.annotate(draft, &text, now()) {
            Ok(id) => {
                tracing::info!(%id, path = %path.display(), %range, "thread started");
                self.refresh_marks(index);
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
                self.refresh_current_marks();
                self.open_thread(id.clone());
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
        if matches!(&self.popup, Some(Popup::Thread(panel)) if panel.id() == id) {
            self.open_thread(id.clone());
        }
        Ok(())
    }

    // ----- thread panel -----

    /// `Space a`: show the thread(s) under the cursor.
    pub fn open_thread_at_cursor(&mut self) {
        let ids = self.threads_at_cursor();
        if ids.is_empty() {
            self.notice("no thread on this line");
            return;
        }
        self.popup = Some(Popup::Thread(ThreadPanel {
            ids,
            index: 0,
            scroll: 0,
        }));
    }

    /// Show one thread; the panel lists only it.
    pub fn open_thread(&mut self, id: ThreadId) {
        self.popup = Some(Popup::Thread(ThreadPanel {
            ids: vec![id],
            index: 0,
            scroll: 0,
        }));
    }

    fn panel_mut(&mut self) -> Option<&mut ThreadPanel> {
        match self.popup.as_mut() {
            Some(Popup::Thread(panel)) => Some(panel),
            _ => None,
        }
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

    /// `r`: reply to the shown thread through the comment box.
    pub fn thread_reply(&mut self) {
        if let Some(Popup::Thread(panel)) = self.popup.take() {
            let id = panel.id().clone();
            self.open_compose(ComposeTarget::Reply(id), Some(panel));
        }
    }

    /// `x`: resolve the shown thread, or reopen it when already resolved.
    pub fn thread_toggle_resolved(&mut self) {
        let Some(id) = self.panel_mut().map(|panel| panel.id().clone()) else {
            return;
        };
        let Some(store) = self.store_mut() else {
            return;
        };
        let open = store
            .thread(&id)
            .is_some_and(|t| t.status() == Status::Open);
        let result = if open {
            store.resolve(&id, Author::User, now())
        } else {
            store.reopen(&id, now())
        };
        match result {
            Ok(()) => {
                tracing::info!(%id, resolved = open, "thread status changed");
                self.refresh_current_marks();
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

    /// `Space A`: pick among the threads of the current document.
    pub fn open_thread_picker(&mut self) {
        let Some(store) = self.store.as_ref() else {
            self.store_mut();
            return;
        };
        let (items, ids): (Vec<String>, Vec<ThreadId>) = self
            .marks()
            .iter()
            .filter_map(|mark| store.thread(mark.id()).map(|t| (t, mark)))
            .map(|(thread, mark)| (describe(thread, mark.placement), mark.id().clone()))
            .unzip();
        if items.is_empty() {
            self.notice("no threads in this file");
            return;
        }
        self.popup = Some(Popup::Picker(
            PickerState::new(PickerKind::Threads, items).with_ids(ids),
        ));
    }

    /// Picker confirm for [`PickerKind::Threads`]: jump there and open it.
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

    use super::{ComposeTarget, MarkKind};
    use crate::app::{App, Popup};

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
            let mut app = App::new(
                workspace,
                100,
                30,
                "test".to_owned(),
                Some(store),
                fathomable_core::config::FollowConfig::default(),
            );
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
                app.compose_newline();
            } else {
                app.compose_char(ch);
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

        // Rewrite them: the thread detaches at its last known range.
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
        let Some(Popup::Thread(panel)) = app.popup() else {
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
        let Some(Popup::Thread(panel)) = app.popup() else {
            anyhow::bail!("panel did not open");
        };
        assert_eq!(panel.position(), (1, 1));
        let id = panel.id().clone();
        app.thread_reply();
        assert!(
            matches!(app.popup(), Some(Popup::Compose(c)) if c.target() == &ComposeTarget::Reply(id.clone()))
        );
        assert!(
            matches!(app.popup(), Some(Popup::Compose(c)) if c.panel().is_some()),
            "the thread stays readable while replying"
        );
        app.compose_scroll(2);
        app.compose_cancel();
        assert!(
            matches!(app.popup(), Some(Popup::Thread(p)) if p.scroll() == 2),
            "Esc returns to the panel"
        );
        app.thread_reply();
        type_in(&mut app, "second thoughts");
        app.compose_submit();
        assert!(
            matches!(app.popup(), Some(Popup::Thread(_))),
            "panel reopens after a reply"
        );
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

        app.open_thread_picker();
        let Some(Popup::Picker(picker)) = app.popup() else {
            anyhow::bail!("picker did not open");
        };
        assert_eq!(picker.matches().len(), 2);
        assert!(picker.item(&picker.matches()[0]).contains("top"));
        app.picker_move(1);
        app.picker_confirm();
        assert!(matches!(app.popup(), Some(Popup::Thread(_))));
        assert_eq!(app.view().cursor_source_line(), bottom);

        app.compose_cancel();
        app.close_popup();
        app.start_comment();
        app.compose_submit();
        assert_eq!(app.message(), Some("empty comment discarded"));
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
}
