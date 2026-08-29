// @okf-doc: /decisions/0036-gutter-rows-and-focus-colour.md
//! The note cell of the gutter (ADR 0027, 0036): a thread's rows are
//! bracketed `╭`, `│`, `╰`, and a thread that fits one row is `•`. The
//! bracket is decided per rendered row from its neighbours, so a
//! one-line thread that wraps over several rows is bracketed like any
//! other range instead of dotting every row.

use fathomable_core::annotations::LineRange;

use super::App;
use super::threads::{MarkKind, overlaps};

impl App {
    /// The note-cell glyph and colour of rendered row `row` of the
    /// current view, or `None` when no thread touches it.
    pub fn note_on_row(&self, row: usize) -> Option<(&'static str, MarkKind)> {
        let view = self.view();
        let lines = view.source_lines_of_row(row)?;
        let above = row.checked_sub(1).and_then(|r| view.source_lines_of_row(r));
        let below = view.source_lines_of_row(row + 1);
        self.note_in(lines, above, below)
    }

    /// The note-cell glyph and colour for a rendered row holding `lines`,
    /// between rows holding `above` and `below` (`None` at the edges of
    /// the document or next to a row with no source): `╭` where a thread
    /// starts, `╰` where one ends, `•` for a thread within the row, `│`
    /// between. Whether a thread starts or ends on the row is whether it
    /// continues onto the neighbouring row, so a wrapped source line is
    /// bracketed across its rows. The thread that starts or ends here
    /// with the shortest range decides the bracket, so a nested thread's
    /// corners sit on the outer thread's line; a bracket beats a dot. The
    /// colour is the most urgent state on the row.
    pub fn note_in(
        &self,
        lines: LineRange,
        above: Option<LineRange>,
        below: Option<LineRange>,
    ) -> Option<(&'static str, MarkKind)> {
        let kind = self.mark_in(lines)?;
        let mut bracket: Option<(usize, &'static str)> = None;
        let mut point = false;
        for mark in self
            .marks()
            .iter()
            .filter(|mark| overlaps(mark.range(), lines))
        {
            let range = mark.range();
            let continues =
                |rows: Option<LineRange>| rows.is_some_and(|rows| overlaps(range, rows));
            let glyph = match (continues(above), continues(below)) {
                (false, false) => {
                    point = true;
                    continue;
                }
                (false, true) => "╭",
                (true, false) => "╰",
                (true, true) => continue,
            };
            if bracket.is_none_or(|(len, _)| range.len() < len) {
                bracket = Some((range.len(), glyph));
            }
        }
        let glyph = match bracket {
            Some((_, glyph)) => glyph,
            None if point => "•",
            None => "│",
        };
        Some((glyph, kind))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::annotations::{LineRange, Store};
    use fathomable_core::workspace::Workspace;

    use crate::app::{App, Options};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str, readme: &str) -> std::io::Result<Self> {
            let dir = std::env::temp_dir()
                .join(format!("fathomable-gutter-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("ws"))?;
            fs::write(dir.join("ws/README.md"), readme)?;
            Ok(Self(dir))
        }

        fn app(&self, width: usize) -> anyhow::Result<App> {
            let workspace = Workspace::discover(self.0.join("ws"))?;
            let store = Store::open(self.0.join("state/threads.jsonl"))?;
            let options = Options {
                store: Some(store),
                ..Options::for_test(self.0.join("ws"))
            };
            let mut app = App::new(workspace, width, 30, options);
            app.open(Path::new("README.md"));
            Ok(app)
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

    /// ADR 0027: the gutter brackets a range, dots a one-row thread, and
    /// re-draws the corners for a nested thread on the outer's line.
    #[test]
    fn the_gutter_brackets_ranges_and_nests_corners() -> anyhow::Result<()> {
        let dir = TempDir::new(
            "brackets",
            "# Readme\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
        )?;
        let mut app = dir.app(100)?;
        // Source view: one row per line, eight lines.
        app.view_mut().toggle_source_view();
        annotate(&mut app, 1, 8, "outer");
        annotate(&mut app, 3, 5, "inner");
        annotate(&mut app, 7, 7, "point");
        assert_eq!(app.thread_counts(), (3, 3));
        let glyphs: String = (0..8)
            .map(|row| app.note_on_row(row).map_or(" ", |(glyph, _)| glyph))
            .collect();
        assert_eq!(glyphs, "╭│╭│╰│•╰");
        assert_eq!(app.note_on_row(8), None);
        // A one-row thread on a range's first row: the bracket wins.
        annotate(&mut app, 1, 1, "on the start");
        assert_eq!(app.note_on_row(0).map(|(g, _)| g), Some("╭"));
        // A row holding the whole inner thread shows it as a dot.
        let row = |line: usize| Some(LineRange::new(line, line));
        let held = app.note_in(LineRange::new(3, 5), row(2), row(6));
        assert_eq!(held.map(|(g, _)| g), Some("•"));
        Ok(())
    }

    /// ADR 0036: a one-line thread whose line wraps over several rendered
    /// rows is bracketed across them, not dotted on each.
    #[test]
    fn a_wrapped_line_is_bracketed_across_its_rows() -> anyhow::Result<()> {
        let long = "word ".repeat(30);
        let dir = TempDir::new("wrapped", &format!("short\n\n{long}\n\nshort\n"))?;
        let mut app = dir.app(40)?;
        let view = app.view();
        let rows: Vec<usize> = (0..view.layout().lines().len())
            .filter(|&row| view.source_lines_of_row(row).is_some_and(|l| l.contains(3)))
            .collect();
        assert!(rows.len() > 2, "the long line wraps: {rows:?}");
        annotate(&mut app, 3, 3, "on the long line");
        let glyphs: String = rows
            .iter()
            .map(|&row| app.note_on_row(row).map_or(" ", |(glyph, _)| glyph))
            .collect();
        let middle = "│".repeat(rows.len() - 2);
        assert_eq!(glyphs, format!("╭{middle}╰"));
        assert_eq!(app.note_on_row(rows[0] - 1), None, "the row above is clear");
        assert_eq!(app.note_on_row(rows[rows.len() - 1] + 1), None);
        Ok(())
    }
}
