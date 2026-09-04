// @okf-doc: /decisions/0039-gutter-colour-and-detached-rows.md
//! Detached threads on rows of their own (ADR 0039). A thread whose
//! lines are gone is not drawn on the lines that now sit at its last
//! known range; the view inserts a blank row where those lines were,
//! and the thread's mark, `c`, and the thread motions find it there.

use super::App;
use super::threads::{Mark, ThreadState};

impl App {
    /// The source line a detached `mark` stands before: the start of its
    /// last known range, clamped to one past the last line of the file.
    pub(super) fn detached_anchor(&self, mark: &Mark) -> usize {
        let past_end = self.view().layout().index().line_count() + 1;
        mark.range().start().min(past_end)
    }

    /// The anchors of the current document's detached threads, for
    /// [`super::view::View::set_detached_anchors`].
    fn detached_anchors(&self) -> Vec<usize> {
        self.marks()
            .iter()
            .filter(|mark| mark.is_detached())
            .map(|mark| self.detached_anchor(mark))
            .collect()
    }

    /// Insert or remove the detached rows of the current document after
    /// its marks changed.
    pub(super) fn place_detached_rows(&mut self) {
        let anchors = self.detached_anchors();
        self.view_mut().set_detached_anchors(anchors);
    }

    /// The detached marks standing before source line `anchor`, oldest
    /// thread first.
    pub(super) fn detached_marks_at(&self, anchor: usize) -> impl Iterator<Item = &Mark> {
        self.marks()
            .iter()
            .filter(move |mark| mark.is_detached() && self.detached_anchor(mark) == anchor)
    }

    /// The note cell of the detached row before `anchor`: a dot in the
    /// colour of the most urgent thread there, or `None` when no thread
    /// stands there any more.
    pub(super) fn detached_note(&self, anchor: usize) -> Option<(&'static str, ThreadState)> {
        self.detached_marks_at(anchor)
            .map(Mark::kind)
            .max()
            .map(|kind| ("•", kind))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::annotations::{LineRange, Store};
    use fathomable_core::workspace::Workspace;

    use crate::app::threads::ThreadState;
    use crate::app::{App, Options, Popup};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str, readme: &str) -> std::io::Result<Self> {
            let dir = std::env::temp_dir()
                .join(format!("fathomable-detached-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("ws"))?;
            fs::write(dir.join("ws/README.md"), readme)?;
            Ok(Self(dir))
        }

        fn app(&self) -> anyhow::Result<App> {
            let workspace = Workspace::discover(self.0.join("ws"))?;
            let store = Store::open(self.0.join("state/threads.jsonl"))?;
            let options = Options {
                store: Some(store),
                ..Options::for_test(self.0.join("ws"))
            };
            let mut app = App::new(workspace, 80, 30, options);
            app.open(Path::new("README.md"));
            Ok(app)
        }

        fn rewrite(&self, readme: &str) -> std::io::Result<()> {
            fs::write(self.0.join("ws/README.md"), readme)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn annotate(app: &mut App, from: usize, to: usize, text: &str) {
        app.view_mut().goto_source_line(from);
        app.view_mut().select_lines();
        app.view_mut().move_down(to - from);
        app.start_new_comment();
        for ch in text.chars() {
            app.compose_insert(&ch.to_string());
        }
        app.compose_submit();
    }

    const BEFORE: &str = "one\n\ntwo\n\nthree\n\nfour\n";

    /// Comment on `two` (line 3), then delete it: the thread detaches
    /// and a blank row appears where the line was.
    fn detach(dir: &TempDir) -> anyhow::Result<App> {
        let mut app = dir.app()?;
        annotate(&mut app, 3, 3, "gone soon");
        dir.rewrite("one\n\nthree\n\nfour\n")?;
        app.on_changes(vec![dir.0.join("ws/README.md")]);
        assert!(app.marks()[0].is_detached());
        Ok(app)
    }

    fn glyphs(app: &App) -> Vec<(usize, Option<usize>, Option<&'static str>)> {
        let rows = app.view().layout().lines().len();
        (0..rows)
            .map(|row| {
                (
                    row,
                    app.view().source_line_of_row(row),
                    app.note_on_row(row).map(|(glyph, _)| glyph),
                )
            })
            .collect()
    }

    #[test]
    fn a_detached_thread_gets_a_row_where_its_lines_were() -> anyhow::Result<()> {
        let dir = TempDir::new("row", BEFORE)?;
        let app = detach(&dir)?;
        let rows = glyphs(&app);
        // one, blank, [detached row], three, blank, four
        assert_eq!(rows[0], (0, Some(1), None));
        assert_eq!(rows[2], (2, None, Some("•")), "{rows:?}");
        assert_eq!(app.view().detached_anchor_of_row(2), Some(3));
        assert_eq!(rows[3], (3, Some(3), None), "the real line 3 is not marked");
        assert_eq!(app.marks()[0].kind(), ThreadState::Open);
        assert_eq!(app.mark_in(LineRange::new(3, 3)), None);
        Ok(())
    }

    #[test]
    fn the_row_is_removed_when_the_thread_re_anchors() -> anyhow::Result<()> {
        let dir = TempDir::new("back", BEFORE)?;
        let mut app = detach(&dir)?;
        dir.rewrite(BEFORE)?;
        app.on_changes(vec![dir.0.join("ws/README.md")]);
        assert!(!app.marks()[0].is_detached());
        assert!(
            app.view()
                .layout()
                .lines()
                .iter()
                .all(|line| line.stands_before().is_none())
        );
        Ok(())
    }

    #[test]
    fn c_on_the_row_opens_the_thread_and_capital_c_is_refused() -> anyhow::Result<()> {
        let dir = TempDir::new("keys", BEFORE)?;
        let mut app = detach(&dir)?;
        let id = app.marks()[0].id().clone();
        app.show_thread(id);
        assert_eq!(app.view().cursor().row, 2);
        app.close_thread();
        assert_eq!(app.threads_at_cursor().len(), 1);
        app.start_new_comment();
        assert!(app.popup().is_none());
        assert_eq!(app.message(), Some("no lines here to annotate"));
        app.start_comment();
        assert!(app.thread_panel().is_some());
        assert!(!matches!(app.popup(), Some(Popup::Compose(_))));
        Ok(())
    }

    #[test]
    fn a_thread_past_the_end_stands_after_the_last_line() -> anyhow::Result<()> {
        let dir = TempDir::new("end", BEFORE)?;
        let mut app = dir.app()?;
        annotate(&mut app, 7, 7, "last");
        dir.rewrite("one\n\ntwo\n")?;
        app.on_changes(vec![dir.0.join("ws/README.md")]);
        assert!(app.marks()[0].is_detached());
        let rows = glyphs(&app);
        let last = rows.last().copied();
        assert_eq!(last.and_then(|(_, _, glyph)| glyph), Some("•"), "{rows:?}");
        assert_eq!(app.view().detached_anchor_of_row(rows.len() - 1), Some(4));
        Ok(())
    }
}
