// @okf-doc: /decisions/0033-open-thread-lines.md
//! The open thread's lines (ADR 0033): the text rows of the thread the
//! cursor is on draw in their own colour, and an agent's reply may say
//! where those lines are now so the thread follows a rewrite it would
//! otherwise have lost.

use fathomable_core::annotations::LineRange;

use crate::app::App;

impl App {
    /// Whether `lines` carries part of the thread the cursor is on: the
    /// thread cursor's, while the text cursor rests on its lines or its
    /// rows (ADR 0049). False when the thread is detached: its last known
    /// range is not its lines, and the gutter already says so.
    pub(crate) fn open_thread_in(&self, lines: LineRange) -> bool {
        let cursor = self.thread_cursor();
        let Some(shown) = cursor.thread() else {
            return false;
        };
        if !self.threads_at_cursor().contains(shown) {
            return false;
        }
        self.placed_marks()
            .filter(|mark| mark.id() == shown)
            .any(|mark| mark.covers(lines))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use fathomable_core::annotations::{Author, LineRange, Placement};
    use fathomable_core::session::{Request, Response};

    use crate::app::testing::{self, source_app};

    fn line(n: usize) -> LineRange {
        LineRange::new(n, n)
    }

    #[test]
    fn the_open_threads_lines_are_marked_unless_detached() -> anyhow::Result<()> {
        let dir = testing::workspace("open-thread-marked", testing::README)?;
        let mut app = source_app(&dir)?;
        app.view_mut().goto_source_line(3);
        app.view_mut().select_lines();
        app.view_mut().move_down(1);
        app.start_comment();
        app.compose_insert("these two");
        app.compose_submit();
        app.expand_at_cursor();
        assert!(app.shows_thread());
        assert!(app.open_thread_in(line(3)));
        assert!(app.open_thread_in(line(4)));
        assert!(!app.open_thread_in(line(5)));
        app.view_mut().goto_source_line(7);
        assert!(
            !app.open_thread_in(line(3)),
            "the cursor elsewhere, nothing marked"
        );

        // The lines vanish: the thread detaches and its last range is
        // no longer claimed.
        fs::write(dir.0.join("ws/README.md"), "# Readme\n\n- one\n- two\n")?;
        app.on_changes(vec![dir.0.join("ws/README.md")]);
        app.threads_pane_open();
        assert!(app.shows_thread());
        assert!(app.marks()[0].is_detached());
        assert!(!app.open_thread_in(line(3)));
        Ok(())
    }

    #[test]
    fn an_agent_reply_with_lines_moves_the_thread() -> anyhow::Result<()> {
        let dir = testing::workspace("open-thread-reply", testing::README)?;
        let mut app = source_app(&dir)?;
        app.view_mut().goto_source_line(3);
        app.start_new_comment();
        app.compose_insert("expand this");
        app.compose_submit();
        let id = app.marks()[0].id().clone();
        // The agent rewrites the whole block, further than the reload
        // diff follows (ADR 0019), and says where the thread belongs now.
        fs::write(
            dir.0.join("ws/README.md"),
            "# Readme\n\nfirst\nsecond\nthird\nfourth\n\n- one\n- two\n",
        )?;
        app.on_changes(vec![dir.0.join("ws/README.md")]);
        assert!(app.marks()[0].is_detached());
        let author = Author::Agent {
            name: "reviewer".to_owned(),
            client: None,
            id: None,
        };
        let reply = app.handle_request(Request::ThreadReply {
            thread: id.clone(),
            author: author.clone(),
            caller: "test:viewer".to_owned(),
            body: "expanded".to_owned(),
            resolve: false,
            lines: Some(LineRange::new(3, 6)),
            idempotency_key: None,
        });
        assert!(matches!(reply, Response::ThreadReply(_)), "{reply:?}");
        assert_eq!(app.marks()[0].range(), Some(LineRange::new(3, 6)));
        assert!(app.marks()[0].placement().is_edited());
        app.threads_pane_open();
        assert!(app.open_thread_in(line(5)));
        assert!(!app.open_thread_in(line(7)));

        // A range past the end of the file is refused, reply and all.
        let reply = app.handle_request(Request::ThreadReply {
            thread: id.clone(),
            author,
            caller: "test:viewer".to_owned(),
            body: "?".to_owned(),
            resolve: false,
            lines: Some(LineRange::new(40, 41)),
            idempotency_key: None,
        });
        assert!(matches!(reply, Response::Error(message) if message.contains("past the end")));
        assert_eq!(app.thread(&id).map(|t| t.replies().len()), Some(1));
        Ok(())
    }

    #[test]
    fn an_agent_reply_at_the_current_lines_keeps_the_anchor() -> anyhow::Result<()> {
        let dir = testing::workspace("open-thread-same-reply", testing::README)?;
        let mut app = source_app(&dir)?;
        app.view_mut().goto_source_line(3);
        app.view_mut().select_lines();
        app.view_mut().move_down(1);
        app.start_comment();
        app.compose_insert("keep this anchor");
        app.compose_submit();
        let id = app.marks()[0].id().clone();

        let reply = app.handle_request(Request::ThreadReply {
            thread: id.clone(),
            author: Author::agent("reviewer"),
            caller: "test:viewer".to_owned(),
            body: "still here".to_owned(),
            resolve: false,
            lines: Some(LineRange::new(3, 4)),
            idempotency_key: None,
        });

        assert!(matches!(reply, Response::ThreadReply(_)), "{reply:?}");
        let thread = app.thread(&id).ok_or_else(|| anyhow::anyhow!("thread"))?;
        assert_eq!(
            thread.locate(testing::README),
            Placement::Anchored(LineRange::new(3, 4))
        );
        assert_eq!(thread.reanchored_at(), None);
        Ok(())
    }
}
