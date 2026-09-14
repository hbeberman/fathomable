// @okf-doc: /decisions/0036-gutter-rows-and-focus-colour.md
//! The note cell of the gutter (ADR 0027, 0036, 0039, 0066): a thread's
//! rows are bracketed `╭`, `│`, `╰`, and a thread that fits one row
//! draws its circle, the one every surface draws.
//! The bracket is decided per rendered row from the nearest rows above
//! and below that have source lines, so a one-line thread that wraps
//! over several rows is bracketed like any other range, and a blank row
//! of rendered markdown inside a thread's lines draws `│`.
//! The git bar bridges sourceless Markdown rows between matching added
//! or modified bars with the same staging state, and extends through
//! trailing synthetic rows to the view's `~` line.

use fathomable_core::annotations::LineRange;
use fathomable_core::diff::LineStatus;

use crate::app::App;
use crate::app::threads::{Mark, ThreadState, overlaps};

impl App {
    /// The git status and staging state drawn on `row`, including
    /// synthetic Markdown rows between matching non-deletion bars or
    /// between one such bar and the view's trailing `~` line.
    pub(super) fn git_on_row(&self, row: usize) -> Option<(LineStatus, bool)> {
        let view = self.view();
        view.layout().lines().get(row)?;
        let status = |line| {
            view.line_status(line)
                .map(|status| (status, view.line_staged(line)))
        };
        if let Some(line) = view.source_line_of_row(row) {
            return status(line);
        }
        if view.source_view()
            || view.diff_view()
            || view.stub_slot_of_row(row).is_some()
            || view.detached_anchor_of_row(row).is_some()
        {
            return None;
        }
        let (above, below) = self.sourced_neighbours(row);
        let above = status(above?.start())?;
        let continues = below.is_none_or(|below| status(below.start()).is_some_and(|s| s == above));
        (continues && above.0 != LineStatus::Removed).then_some(above)
    }

    /// The note-cell glyph and colour of rendered row `row` of the
    /// current view, or `None` when no thread touches it: the bracket
    /// of the threads on its lines, `│` on a sourceless row a thread
    /// spans, `?` on a detached thread's row (ADR 0039, ADR 0066).
    pub(crate) fn note_on_row(&self, row: usize) -> Option<(&'static str, ThreadState)> {
        let view = self.view();
        if let Some(anchor) = view.detached_anchor_of_row(row) {
            return self.detached_note(anchor);
        }
        let (above, below) = self.sourced_neighbours(row);
        match view.source_lines_of_row(row) {
            Some(lines) => self.note_in(lines, above, below),
            None => self.spanning_mark(above?, below?).map(|kind| ("│", kind)),
        }
    }

    /// Whether the thread cursor's thread (the focused thread, ADR 0033)
    /// covers rendered row `row`:
    /// its lines, or a sourceless row between two of them.
    pub(crate) fn open_thread_on_row(&self, row: usize) -> bool {
        if let Some(lines) = self.view().source_lines_of_row(row) {
            return self.open_thread_in(lines);
        }
        let (above, below) = self.sourced_neighbours(row);
        above.is_some_and(|above| self.open_thread_in(above))
            && below.is_some_and(|below| self.open_thread_in(below))
    }

    /// The source lines of the nearest rows above and below `row` that
    /// have any, `None` at the edges of the document.
    fn sourced_neighbours(&self, row: usize) -> (Option<LineRange>, Option<LineRange>) {
        let view = self.view();
        let rows = view.layout().lines().len();
        let above = (0..row).rev().find_map(|r| view.source_lines_of_row(r));
        let below = (row + 1..rows).find_map(|r| view.source_lines_of_row(r));
        (above, below)
    }

    /// The most urgent thread covering both `above` and `below`.
    fn spanning_mark(&self, above: LineRange, below: LineRange) -> Option<ThreadState> {
        self.placed_marks()
            .filter(|mark| mark.covers(above) && mark.covers(below))
            .map(Mark::kind)
            .max()
    }

    /// The note-cell glyph and colour for a rendered row holding `lines`,
    /// between rows holding `above` and `below` (`None` at the edges of
    /// the document or next to a row with no source): `╭` where a thread
    /// starts, `╰` where one ends, the thread's circle for a thread
    /// within the row (ADR 0066), `│` between. Whether a thread starts or ends on the row is whether it
    /// continues onto the neighbouring row, so a wrapped source line is
    /// bracketed across its rows. The thread that starts or ends here
    /// with the shortest range decides the bracket, so a nested thread's
    /// corners sit on the outer thread's line; a bracket beats a dot. The
    /// colour is the most urgent state on the row.
    pub(crate) fn note_in(
        &self,
        lines: LineRange,
        above: Option<LineRange>,
        below: Option<LineRange>,
    ) -> Option<(&'static str, ThreadState)> {
        let kind = self.mark_in(lines)?;
        let mut bracket: Option<(usize, &'static str)> = None;
        // The most urgent one-row thread's circle, when a bracket does
        // not take the cell.
        let mut point: Option<((ThreadState, u8), &'static str)> = None;
        for mark in self.placed_marks().filter(|mark| mark.covers(lines)) {
            let Some(range) = mark.range() else {
                continue;
            };
            let continues =
                |rows: Option<LineRange>| rows.is_some_and(|rows| overlaps(range, rows));
            let glyph = match (continues(above), continues(below)) {
                (false, false) => {
                    let urgency = mark.words().urgency();
                    if point.is_none_or(|(loudest, _)| urgency > loudest) {
                        point = Some((urgency, mark.glyph()));
                    }
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
            None => point.map_or("│", |(_, glyph)| glyph),
        };
        Some((glyph, kind))
    }
}

#[cfg(test)]
mod tests {

    use anyhow::Context as _;
    use crossterm::event::KeyCode;
    use fathomable_core::annotations::LineRange;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Span;

    use crate::app::App;
    use crate::app::draw::{Theme, gutter_width, text_lines};
    use fathomable_testing::TempDir;

    use crate::app::testing::{self, AppBuilder};

    fn app(dir: &TempDir, width: usize) -> anyhow::Result<App> {
        AppBuilder::new(dir).width(width).build()
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

    fn git_cells(app: &App) -> anyhow::Result<Vec<Span<'_>>> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        Ok(text_lines(
            app,
            &theme,
            gutter_width(app.view()),
            app.view().layout().lines().len() - app.view().scroll(),
        )
        .into_iter()
        .map(|line| line.spans[3].clone())
        .collect())
    }

    fn number_cells(app: &App, theme: &Theme) -> Vec<Span<'static>> {
        text_lines(
            app,
            theme,
            gutter_width(app.view()),
            app.view().layout().lines().len() - app.view().scroll(),
        )
        .into_iter()
        .filter_map(|line| line.spans.into_iter().nth(1))
        .map(|span| Span::styled(span.content.into_owned(), span.style))
        .collect()
    }

    #[test]
    fn current_line_number_uses_the_active_cursor_colour_only_while_reading() -> anyhow::Result<()>
    {
        let dir = testing::workspace("gutter-current-number", "first\nsecond\nthird\n")?;
        for name in fathomable_core::theme::BUILTIN_NAMES {
            let core = fathomable_core::theme::Theme::resolve(name, |_| Ok(None))?;
            let mut theme = Theme::from_core(&core);
            let mut app = testing::source_app(&dir)?;
            app.view_mut().goto_source_line(2);
            let cells = number_cells(&app, &theme);
            assert_eq!(cells[1].content.trim(), "2");
            assert_eq!(cells[1].style.fg, theme.list_cursor.fg);
            assert_ne!(cells[1].style.fg, theme.line_number.fg);
            assert_eq!(cells[0].style, theme.line_number);
            assert_eq!(cells[2].style, theme.line_number);
            assert_eq!(cells[1].style.bg, theme.line_number.bg);

            testing::press(&mut app, "j");
            let cells = number_cells(&app, &theme);
            assert_eq!(cells[1].style, theme.line_number);
            assert_eq!(cells[2].style.fg, theme.list_cursor.fg);
            testing::press(&mut app, "V");
            assert_eq!(number_cells(&app, &theme)[2].style.fg, theme.list_cursor.fg);
            testing::press_key(&mut app, KeyCode::Esc);

            // Theme accents cannot introduce a line-number background or
            // inherit the inactive number's dim modifier.
            theme.line_number = theme
                .line_number
                .bg(Color::Black)
                .add_modifier(Modifier::DIM);
            theme.list_cursor = Style::default().fg(Color::White).bg(Color::Blue);
            let active = number_cells(&app, &theme)[2].style;
            assert_eq!(active.fg, Some(Color::White));
            assert_eq!(active.bg, Some(Color::Black));
            assert!(!active.add_modifier.contains(Modifier::DIM));

            for keys in [" ", " ?", " f", ":", "/"] {
                testing::press(&mut app, keys);
                assert!(
                    number_cells(&app, &theme)
                        .iter()
                        .all(|cell| cell.style == theme.line_number)
                );
                testing::press_key(&mut app, KeyCode::Esc);
                assert_eq!(number_cells(&app, &theme)[2].style.fg, theme.list_cursor.fg);
            }
            app.window_files();
            assert!(
                number_cells(&app, &theme)
                    .iter()
                    .all(|cell| cell.style == theme.line_number)
            );
            app.focus = crate::app::Focus::View;
            assert_eq!(number_cells(&app, &theme)[2].style.fg, theme.list_cursor.fg);
        }
        Ok(())
    }

    #[test]
    fn current_number_follows_wrapping_but_not_synthetic_or_removed_rows() -> anyhow::Result<()> {
        let dir = testing::workspace(
            "gutter-wrapped-number",
            "a paragraph with enough words to wrap over several rendered rows\n\nsecond paragraph\n",
        )?;
        let mut app = app(&dir, 25)?;
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        app.view_mut().move_down(1);
        assert_eq!(
            app.view().source_line_of_row(app.view().cursor().row),
            Some(1)
        );
        assert_eq!(
            app.view().layout().lines()[app.view().cursor().row].source_line(),
            None
        );
        let cells = number_cells(&app, &theme);
        assert_eq!(cells[0].content.trim(), "1");
        assert_eq!(cells[0].style.fg, theme.list_cursor.fg);
        assert_eq!(cells[1].content.trim(), "");
        assert_eq!(cells[1].style, theme.line_number);

        while app
            .view()
            .source_line_of_row(app.view().cursor().row)
            .is_some()
        {
            app.view_mut().move_down(1);
        }
        assert!(
            number_cells(&app, &theme)
                .iter()
                .all(|cell| cell.style == theme.line_number)
        );

        app.view_mut()
            .set_bases(None, None, Some("old paragraph\n".to_owned()));
        app.view_mut().toggle_head_diff();
        app.view_mut().goto_top();
        let rows = app.view().layout().lines().len();
        let mut numbered = 0;
        let mut unnumbered = 0;
        for _ in 0..rows {
            let source = app.view().source_line_of_row(app.view().cursor().row);
            let cells = number_cells(&app, &theme);
            if let Some(source) = source {
                numbered += 1;
                assert!(cells.iter().any(|cell| {
                    cell.content.trim() == source.to_string()
                        && cell.style.fg == theme.list_cursor.fg
                }));
            } else {
                unnumbered += 1;
                assert!(cells.iter().all(|cell| cell.style == theme.line_number));
            }
            app.view_mut().move_down(1);
        }
        assert!(numbered > 0);
        assert!(unnumbered > 0);
        Ok(())
    }

    #[test]
    fn git_bars_bridge_markdown_spacing_with_matching_colour_and_thickness() -> anyhow::Result<()> {
        let text = "first\n\nsecond\n";
        let dir = testing::workspace("git-gutter-spacing", text)?;
        let mut app = app(&dir, 100)?;
        for (head, index, glyph) in [
            ("", "", "▎"),
            ("", text, "▌"),
            ("old first\n\nold second\n", "", "▎"),
            ("old first\n\nold second\n", text, "▌"),
        ] {
            app.view_mut()
                .set_bases(None, Some(index.to_owned()), Some(head.to_owned()));
            let cells = git_cells(&app)?;
            assert_eq!(app.view().source_line_of_row(1), None);
            assert_eq!(cells[0].content, glyph);
            assert_eq!(cells[1], cells[0], "gap keeps colour and thickness");
            assert_eq!(cells[2], cells[0]);
        }
        Ok(())
    }

    #[test]
    fn git_bars_leave_status_boundaries_and_deletion_ticks_separate() -> anyhow::Result<()> {
        let text = "first\n\nsecond\n";
        let dir = testing::workspace("git-gutter-boundaries", text)?;
        let mut app = app(&dir, 100)?;
        for (head, index, expected) in [
            ("", "first\n", ["▌", " ", "▎"]),
            ("", "second\n", ["▎", " ", "▌"]),
            ("first\n", "", [" ", " ", "▎"]),
            ("second\n", "", ["▎", " ", " "]),
            ("old first\n\n", "", ["▎", " ", "▎"]),
            (
                "removed\nfirst\n\nremoved too\nsecond\n",
                "",
                ["▔", " ", "▔"],
            ),
            (text, text, [" ", " ", " "]),
        ] {
            app.view_mut()
                .set_bases(None, Some(index.to_owned()), Some(head.to_owned()));
            let cells = git_cells(&app)?;
            let glyphs: Vec<_> = cells
                .iter()
                .take(3)
                .map(|cell| cell.content.as_ref())
                .collect();
            assert_eq!(glyphs, expected, "head: {head:?}, index: {index:?}");
        }
        app.view_mut().set_bases(None, None, None);
        assert!(git_cells(&app)?.iter().all(|cell| cell.content == " "));
        Ok(())
    }

    #[test]
    fn git_bars_bridge_table_borders_through_the_trailing_edge() -> anyhow::Result<()> {
        let dir = testing::workspace("git-gutter-table", "| key |\n| --- |\n| a |\n| b |\n")?;
        let mut app = app(&dir, 100)?;
        app.view_mut()
            .set_bases(None, Some(String::new()), Some(String::new()));
        let cells = git_cells(&app)?;
        assert_eq!(cells.first().context("table top border")?.content, " ");
        assert_eq!(cells.last().context("table bottom border")?.content, "▎");
        assert!(cells[1..].iter().all(|cell| cell.content == "▎"));
        let synthetic = app
            .view()
            .layout()
            .lines()
            .iter()
            .enumerate()
            .filter(|(row, _)| app.view().source_line_of_row(*row).is_none())
            .map(|(row, line)| (row, line.text()))
            .collect::<Vec<_>>();
        assert!(synthetic.len() >= 4, "{synthetic:?}");
        assert_eq!(
            synthetic.last().context("trailing synthetic row")?.0,
            cells.len() - 1
        );
        // The `~` approves the synthetic rows above it; it does not carry
        // a bar itself and remains outside the document layout.
        assert_eq!(app.git_on_row(cells.len()), None);
        Ok(())
    }

    #[test]
    fn git_bars_stay_continuous_through_wrapping_and_consecutive_synthetic_rows()
    -> anyhow::Result<()> {
        let text = format!(
            "{}\n\n| key |\n| --- |\n| value |\n\nafter\n",
            "wrapped words ".repeat(20)
        );
        let dir = testing::workspace("git-gutter-continuous", &text)?;
        let mut app = app(&dir, 100)?;
        app.view_mut()
            .set_bases(None, Some(String::new()), Some(String::new()));
        let lines = app.view().layout().lines();
        assert!(
            lines
                .windows(2)
                .any(|pair| { pair[0].source().is_none() && pair[1].source().is_none() })
        );
        assert!(
            (0..lines.len())
                .filter(|&row| app.view().source_line_of_row(row) == Some(1))
                .count()
                > 1
        );
        assert!(git_cells(&app)?.iter().all(|cell| cell.content == "▎"));
        Ok(())
    }

    #[test]
    fn git_bars_leave_unchanged_code_blanks_and_thread_rows_unmarked() -> anyhow::Result<()> {
        let dir = testing::workspace("git-gutter-code-blank", "```\nfirst\n\nsecond\n```\n")?;
        let mut app = app(&dir, 100)?;
        app.view_mut().set_bases(
            None,
            Some(String::new()),
            Some("```\nold first\n\nold second\n```\n".to_owned()),
        );
        let blank = (0..app.view().layout().lines().len())
            .find(|&row| app.view().source_line_of_row(row) == Some(3))
            .context("source-backed blank code line")?;
        assert_eq!(git_cells(&app)?[blank].content, " ");

        app.view_mut()
            .set_bases(None, Some(String::new()), Some(String::new()));
        annotate(&mut app, 2, 2, "a thread");
        let stub = (0..app.view().layout().lines().len())
            .find(|&row| app.view().stub_slot_of_row(row).is_some())
            .context("thread stub row")?;
        assert_eq!(app.git_on_row(stub), None);
        app.view_mut().set_detached_anchors(vec![4]);
        let detached = (0..app.view().layout().lines().len())
            .find(|&row| app.view().detached_anchor_of_row(row).is_some())
            .context("detached thread row")?;
        assert_eq!(app.git_on_row(detached), None);
        Ok(())
    }

    #[test]
    fn git_gap_bars_do_not_depend_on_visible_neighbours() -> anyhow::Result<()> {
        let dir = testing::workspace("git-gutter-scroll", "first\n\nsecond\n\nthird\n")?;
        let mut app = app(&dir, 100)?;
        app.view_mut()
            .set_bases(None, Some(String::new()), Some(String::new()));
        let before = git_cells(&app)?[1].style;
        let width = app.view().layout().width();
        app.view_mut().resize(width, 1);
        app.view_mut().scroll_by(1);
        assert_eq!(app.view().scroll(), 1);
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = Theme::from_core(&core);
        let visible = text_lines(&app, &theme, gutter_width(app.view()), 1);
        assert_eq!(visible[0].spans[3].style, before);
        assert_eq!(visible[0].spans[3].content, "▎");
        Ok(())
    }

    #[test]
    fn git_gap_bars_do_not_change_source_or_unified_diff_rows() -> anyhow::Result<()> {
        let dir = testing::workspace("git-gutter-displays", "first\n\nsecond\n")?;
        let mut app = app(&dir, 100)?;
        app.view_mut().set_bases(
            None,
            Some(String::new()),
            Some("old first\n\nold second\n".to_owned()),
        );
        app.view_mut().toggle_source_view();
        assert_eq!(app.view().source_line_of_row(1), Some(2));
        assert_eq!(git_cells(&app)?[1].content, " ");

        app.view_mut().toggle_head_diff();
        let cells = git_cells(&app)?;
        let mut synthetic = 0;
        for (row, cell) in cells.iter().enumerate() {
            if app.view().source_line_of_row(row).is_none() {
                synthetic += 1;
                assert_eq!(cell.content, " ", "diff row {row}");
            }
        }
        assert!(synthetic >= 3, "hunk header and removed lines");
        Ok(())
    }

    /// ADR 0027: the gutter brackets a range, dots a one-row thread, and
    /// re-draws the corners for a nested thread on the outer's line.
    #[test]
    fn the_gutter_brackets_ranges_and_nests_corners() -> anyhow::Result<()> {
        let dir = testing::workspace(
            "gutter-brackets",
            "# Readme\n\nalpha\nbeta\ngamma\n\n- one\n- two\n",
        )?;
        let mut app = app(&dir, 100)?;
        // Source view: one row per line, eight lines.
        app.view_mut().toggle_source_view();
        annotate(&mut app, 1, 8, "outer");
        annotate(&mut app, 3, 5, "inner");
        annotate(&mut app, 7, 7, "point");
        assert_eq!(app.thread_counts(), (3, 3));
        // Stub rows (ADR 0049) sit between; the document's rows are read.
        let document_rows = |app: &App| -> Vec<usize> {
            (0..app.view().layout().lines().len())
                .filter(|&row| app.view().stub_slot_of_row(row).is_none())
                .collect()
        };
        let glyphs: String = document_rows(&app)
            .into_iter()
            .map(|row| app.note_on_row(row).map_or(" ", |(glyph, _)| glyph))
            .collect();
        assert_eq!(glyphs, "╭│╭│╰│●╰");
        assert_eq!(app.note_on_row(app.view().layout().lines().len()), None);
        // A one-row thread on a range's first row: the bracket wins.
        annotate(&mut app, 1, 1, "on the start");
        assert_eq!(app.note_on_row(0).map(|(g, _)| g), Some("╭"));
        // A row holding the whole inner thread shows it as a dot.
        let row = |line: usize| Some(LineRange::new(line, line));
        let held = app.note_in(LineRange::new(3, 5), row(2), row(6));
        assert_eq!(held.map(|(g, _)| g), Some("●"));
        Ok(())
    }

    /// ADR 0036: a one-line thread whose line wraps over several rendered
    /// rows is bracketed across them, not dotted on each.
    #[test]
    fn a_wrapped_line_is_bracketed_across_its_rows() -> anyhow::Result<()> {
        let long = "word ".repeat(30);
        let dir = testing::workspace("gutter-wrapped", &format!("short\n\n{long}\n\nshort\n"))?;
        let mut app = app(&dir, 40)?;
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

    /// ADR 0039: the blank rows rendered markdown puts between paragraphs
    /// do not break a bracket; they draw `│` inside it.
    #[test]
    fn blank_rows_inside_a_range_are_bridged() -> anyhow::Result<()> {
        let dir = testing::workspace("gutter-bridged", "# Title\n\none\n\ntwo\n\nthree\n\nfour\n")?;
        let mut app = app(&dir, 60)?;
        // Lines 3 to 7: `one`, `two`, `three`, with blank lines between.
        annotate(&mut app, 3, 7, "range");
        annotate(&mut app, 3, 3, "point on the start");
        let view = app.view();
        let rows = view.layout().lines().len();
        let glyphs: String = (0..rows)
            .filter(|&row| view.stub_slot_of_row(row).is_none())
            .map(|row| app.note_on_row(row).map_or(" ", |(glyph, _)| glyph))
            .collect();
        assert_eq!(glyphs.trim_end(), "  ╭│││╰", "{glyphs:?}");
        // The thread the cursor is on tints its rows (ADR 0049); elsewhere
        // nothing is tinted.
        assert!(app.open_thread_on_row(2));
        app.view_mut().goto_source_line(1);
        assert!(!app.open_thread_on_row(2));
        Ok(())
    }
}
