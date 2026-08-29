// @okf-doc: /decisions/0033-open-thread-lines.md
//! The open thread's lines (ADR 0033): the text rows the thread pane is
//! about draw in their own colour, and an agent's reply may say where
//! those lines are now so the thread follows a rewrite it would
//! otherwise have lost.

use std::fs;
use std::path::Path;

use fathomable_core::annotations::{LineRange, Store, ThreadId};

use super::App;
use super::threads::overlaps;

impl App {
    /// Whether `lines` carries part of the open thread pane's thread.
    /// False when no pane is open or the thread is detached: its last
    /// known range is not its lines, and the gutter already says so.
    pub fn open_thread_in(&self, lines: LineRange) -> bool {
        let Some(panel) = self.thread.as_ref() else {
            return false;
        };
        self.placed_marks()
            .filter(|mark| mark.id() == panel.id())
            .any(|mark| overlaps(mark.range(), lines))
    }
}

/// Re-anchor `id` onto `lines` of its file as it is on disk, because an
/// agent's reply said so. The file is read from disk, not from the
/// viewer's document: the agent speaks of the text it just wrote, which
/// the viewer may not have reloaded yet.
///
/// The move is a `relocate` event (ADR 0019), so the thread shows as
/// *edited* until the user answers, exactly as a rewrite the reload diff
/// followed would.
pub(crate) fn follow_reply_lines(
    store: &mut Store,
    root: &Path,
    id: &ThreadId,
    lines: LineRange,
    when: u64,
) -> Result<(), String> {
    let path = store
        .thread(id)
        .map(|thread| thread.path().to_path_buf())
        .ok_or_else(|| format!("unknown thread {id}"))?;
    let text = fs::read_to_string(root.join(&path))
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    store
        .relocate(id, lines, &text, when)
        .map_err(|error| format!("cannot move {id} to {lines}: {error}"))?;
    tracing::info!(%id, path = %path.display(), %lines, "thread re-anchored by an agent's reply");
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::annotations::{Author, LineRange, Store};
    use fathomable_core::session::{Request, Response};
    use fathomable_core::workspace::Workspace;

    use crate::app::{App, Options};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> std::io::Result<Self> {
            let dir = std::env::temp_dir().join(format!(
                "fathomable-open-thread-{name}-{}",
                std::process::id()
            ));
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
            app.view_mut().toggle_source_view();
            Ok(app)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn line(n: usize) -> LineRange {
        LineRange::new(n, n)
    }

    #[test]
    fn the_open_threads_lines_are_marked_unless_detached() -> anyhow::Result<()> {
        let dir = TempDir::new("marked")?;
        let mut app = dir.app()?;
        app.view_mut().goto_source_line(3);
        app.view_mut().select_lines();
        app.view_mut().move_down(1);
        app.start_comment();
        app.compose_insert("these two");
        app.compose_submit();
        app.open_thread_at_cursor();
        assert!(app.thread_panel().is_some());
        assert!(app.open_thread_in(line(3)));
        assert!(app.open_thread_in(line(4)));
        assert!(!app.open_thread_in(line(5)));
        app.close_thread();
        assert!(!app.open_thread_in(line(3)), "nothing open, nothing marked");

        // The lines vanish: the thread detaches and its last range is
        // no longer claimed.
        fs::write(dir.0.join("ws/README.md"), "# Readme\n\n- one\n- two\n")?;
        app.on_changes(vec![dir.0.join("ws/README.md")]);
        app.file_thread_open();
        assert!(app.thread_panel().is_some());
        assert!(app.marks()[0].is_detached());
        assert!(!app.open_thread_in(line(3)));
        Ok(())
    }

    #[test]
    fn an_agent_reply_with_lines_moves_the_thread() -> anyhow::Result<()> {
        let dir = TempDir::new("reply")?;
        let mut app = dir.app()?;
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
        };
        let reply = app.handle_request(Request::ThreadReply {
            thread: id.clone(),
            author: author.clone(),
            body: "expanded".to_owned(),
            resolve: false,
            lines: Some(LineRange::new(3, 6)),
        });
        assert_eq!(reply, Response::Done);
        assert_eq!(app.marks()[0].range(), LineRange::new(3, 6));
        assert!(app.marks()[0].placement().is_edited());
        app.file_thread_open();
        assert!(app.open_thread_in(line(5)));
        assert!(!app.open_thread_in(line(7)));

        // A range past the end of the file is refused, reply and all.
        let reply = app.handle_request(Request::ThreadReply {
            thread: id.clone(),
            author,
            body: "?".to_owned(),
            resolve: false,
            lines: Some(LineRange::new(40, 41)),
        });
        assert!(matches!(reply, Response::Error(message) if message.contains("cannot move")));
        assert_eq!(app.thread(&id).map(|t| t.replies().len()), Some(1));
        Ok(())
    }
}
