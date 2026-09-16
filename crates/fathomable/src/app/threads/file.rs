// @okf-doc: /decisions/0063-a-comment-on-the-file.md
//! A comment on the file as a whole (ADR 0063).
//!
//! A thread need not be on lines: `Space c f` writes a comment on the
//! open file itself, and an agent's `thread_start` does the same by
//! naming no line. Such a thread has no range, no anchor, and no
//! snippet; it is never edited, detached, or re-anchored; its stub
//! stands above the first line (`RowAnchor::Top`), its placement word
//! is `file`, and every list names it by its path alone and puts it
//! before the file's line threads.

use std::path::Path;

use fathomable_core::annotations::{LineRange, Thread};

use crate::app::App;
use crate::app::threads::ComposeTarget;

/// Where a toast says `thread` is: `path:line` for a thread on lines,
/// the path alone for one on the file as a whole.
pub(crate) fn toast_place(thread: &Thread) -> String {
    toast_place_at(thread.path(), thread.range())
}

/// Where a toast names a durable activity location.
pub(crate) fn toast_place_at(path: &Path, range: Option<LineRange>) -> String {
    match range {
        Some(range) => format!("{}:{}", path.display(), range.start()),
        None => path.display().to_string(),
    }
}

impl App {
    /// `Space c f`: a draft on the open file as a whole, written in a
    /// block above the first line.
    pub(crate) fn start_file_comment(&mut self) {
        if !self.can_annotate() {
            return;
        }
        self.open_compose(ComposeTarget::OnFile);
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use fathomable_core::annotations::{Author, LineRange, Placement};
    use fathomable_core::layout::RowAnchor;
    use fathomable_core::session::{Request, Response};

    use crate::app::testing::{self, press, screen, source_app};
    use crate::app::threads::list::Row;
    use crate::app::threads::pane::PaneRow;
    use crate::app::threads::stubs::{Stub, Subject};
    use crate::app::threads::{Compose, ComposeTarget};
    use crate::app::{App, Popup};

    fn annotate(app: &mut App, line: usize, text: &str) {
        app.view_mut().goto_source_line(line);
        app.start_new_comment();
        app.compose_insert(text);
        app.compose_submit();
    }

    /// `Space c f` writes a comment on the file in a block above the
    /// first line; the thread has no lines, stands above L1 as a stub,
    /// says `file` where another says `detached`, comes first in every
    /// order, and folds on `z`.
    #[test]
    fn space_c_f_comments_on_the_file_as_a_whole() -> anyhow::Result<()> {
        let dir = testing::workspace("file-comment", testing::README)?;
        let mut app = source_app(&dir)?;
        annotate(&mut app, 3, "on alpha");
        app.view_mut().goto_source_line(5);
        press(&mut app, " cf");
        assert!(matches!(
            app.draft().map(Compose::target),
            Some(ComposeTarget::OnFile)
        ));
        let stubs = app.stubs();
        assert!(matches!(
            stubs.last().map(Stub::subject),
            Some(Subject::FileDraft)
        ));
        let shown = screen(&app)?;
        assert!(
            shown[0].contains("comment on README.md"),
            "the draft block heads the file: {:?}",
            &shown[..3]
        );
        app.compose_insert("split this file");
        app.compose_submit();
        assert!(!matches!(app.popup(), Some(Popup::Compose(_))));
        assert_eq!(app.message(), Some("commented on the file"));

        let file_thread = app.file_threads()[0].clone();
        let thread = app
            .thread(&file_thread)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("gone"))?;
        assert_eq!(thread.range(), None);
        assert!(thread.is_on_file());
        assert_eq!(thread.snippet(), "");
        assert_eq!(thread.comment(), "split this file");
        assert_eq!(thread.place(), "README.md");
        let mark = app
            .marks()
            .iter()
            .find(|mark| *mark.id() == file_thread)
            .cloned();
        assert!(matches!(
            mark.map(|mark| mark.placement()),
            Some(Placement::File)
        ));
        // Its stub stands above L1, before the L3 thread's.
        let stubs = app.stubs();
        assert_eq!(stubs[0].thread(), Some(&file_thread));
        assert_eq!(stubs[0].block().anchor, RowAnchor::Top);
        assert!(app.view().stub_slot_of_row(0).is_some(), "a stub row first");
        assert_eq!(app.view().source_line_of_row(0), None);
        assert_ne!(
            app.view().cursor().row,
            0,
            "the cursor never rests on a folded stub"
        );
        // The gutter carries no bracket for it: L1 shows no mark.
        assert_eq!(app.mark_in(LineRange::new(1, 1)), None);

        // Expanded with the cursor on its comment, its header says `file`
        // and the state; `c` replies, while `z` folds it.
        app.goto_message(file_thread.clone(), 0);
        assert_eq!(app.thread_cursor().thread(), Some(&file_thread));
        let shown = screen(&app)?;
        assert!(
            shown[0].contains("file") && shown[0].contains("Resolve"),
            "{:?}",
            &shown[..4]
        );
        press(&mut app, "c");
        assert!(app.is_expanded(&file_thread));
        assert!(matches!(
            app.draft().map(Compose::target),
            Some(ComposeTarget::Reply(id)) if id == &file_thread
        ));
        app.compose_cancel();
        press(&mut app, "z");
        assert!(!app.is_expanded(&file_thread));

        // The lists name it by the path alone and put it first.
        app.open_review();
        let rows = app.review_rows(100);
        assert!(matches!(
            rows.rows.get(1),
            Some(Row::Header { summary, .. })
                if summary.location() == "file"
                    && summary.lifecycle() == fathomable_core::annotations::Lifecycle::Active
        ));
        app.close_review();
        let pane = app.threads_pane_entries();
        assert_eq!(pane[0].place(), "file");
        assert_eq!(pane[1].place(), "L3");
        app.threads_pane_toggle_scope();
        assert!(matches!(
            app.threads_pane_rows().first(),
            Some(PaneRow::File { path, count: 2, .. }) if path == Path::new("README.md")
        ));
        Ok(())
    }

    /// An agent's `thread_start` with no line is a comment on the file
    /// (ADR 0063): the thread has no range, and the toast names the path.
    #[test]
    fn an_agent_starts_a_file_thread_by_naming_no_line() -> anyhow::Result<()> {
        let dir = testing::workspace("file-comment-agent", testing::README)?;
        let mut app = testing::app(&dir)?;
        let author = Author::agent("reviewer");
        let reply = app.handle_request(Request::ThreadStart {
            path: PathBuf::from("README.md"),
            range: None,
            author: author.clone(),
            caller: "test:viewer".to_owned(),
            body: "rename this".to_owned(),
            idempotency_key: None,
        });
        let Response::Threads(started) = reply else {
            anyhow::bail!("start answered {reply:?}");
        };
        assert_eq!(started[0].range(), None);
        assert_eq!(started[0].author(), &author);
        assert_eq!(
            started[0].lifecycle(),
            fathomable_core::annotations::Lifecycle::Active
        );
        assert_eq!(
            app.toasts().last().map(crate::app::Toast::text),
            Some("reviewer started a thread on README.md")
        );
        Ok(())
    }
}
