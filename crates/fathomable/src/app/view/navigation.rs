// @okf-doc: /decisions/0010-viewer-ux.md
//! Navigation through source lines across their rendered wraps.

use fathomable_core::link;

use super::{Cursor, View};

#[derive(Clone, Copy)]
enum Edge {
    Start,
    End,
}

impl View {
    /// The rendered link destination, otherwise the source word under the
    /// cursor, including portions wrapped onto other screen rows.
    pub(crate) fn file_reference(&self) -> Option<String> {
        if let Some(url) = self.link_at_cursor() {
            return Some(url.to_owned());
        }
        if let Some(offset) = self.cursor_offset() {
            return link::word_at(self.shown(), offset).map(str::to_owned);
        }
        let line = self.layout.lines().get(self.cursor.row)?;
        let text = line.text();
        let byte = line.byte_at(self.cursor.col)?;
        link::word_at(&text, byte).map(str::to_owned)
    }

    /// `gh`: first displayed character of the source line, including indentation.
    pub(crate) fn goto_line_start(&mut self) {
        self.goto_line_edge(Edge::Start);
    }

    /// `gl`: last displayed grapheme of the source line, across wrapped rows.
    pub(crate) fn goto_line_end(&mut self) {
        self.goto_line_edge(Edge::End);
    }

    fn goto_line_edge(&mut self, edge: Edge) {
        if let Some(cursor) = self.source_line_edge(edge) {
            self.cursor = cursor;
        } else {
            // Synthesized and thread rows have no source line to traverse.
            match edge {
                Edge::Start => self.line_start(),
                Edge::End => self.line_end(),
            }
        }
        self.want_col = match edge {
            Edge::Start => self.cursor.col,
            Edge::End => usize::MAX,
        };
        self.extend_selection();
        self.ensure_visible();
    }

    fn source_line_edge(&self, edge: Edge) -> Option<Cursor> {
        let offset = self.cursor_offset()?;
        let index = self.layout.index();
        let range = index.range_of(index.line_of(offset))?;
        let offset = match edge {
            Edge::Start => range.start,
            Edge::End => range.end.saturating_sub(1).max(range.start),
        };
        let row = self.layout.line_at_offset(offset)?;
        let line = &self.layout.lines()[row];
        let columns = line.columns();
        let col = match edge {
            Edge::Start => columns
                .iter()
                .copied()
                .find(|&col| line.source_at(col).is_some_and(|at| at >= range.start))
                .unwrap_or(0),
            Edge::End => columns
                .iter()
                .copied()
                .rev()
                .find(|&col| line.source_at(col).is_some_and(|at| at < range.end))
                .unwrap_or(0),
        };
        Some(Cursor { row, col })
    }
}

#[cfg(test)]
mod tests {
    use super::View;
    use crate::app::testing::{self, press};
    use crate::app::view::{Cursor, Mode, StubBlock};
    use fathomable_core::layout::RowAnchor;

    #[test]
    fn goto_line_keys_cross_wraps_and_extend_a_selection() -> anyhow::Result<()> {
        let text = "    abcdefghijklmnopqrstuvwxyz\nnext\n";
        let dir = testing::workspace("goto-line", text)?;
        let mut app = testing::AppBuilder::new(&dir).source_view().build()?;
        app.resize(24, 8);
        assert!(app.view().layout().lines().len() > 2);
        press(&mut app, "gl");
        assert_eq!(app.view().source_position(), (1, 29));
        assert!(app.view().cursor().row > 0);
        press(&mut app, "vgh");
        assert_eq!(app.view().source_position(), (1, 0));
        assert_eq!(app.view().mode(), Mode::Select);
        assert_eq!(
            app.view().selected_source().as_deref(),
            Some("    abcdefghijklmnopqrstuvwxyz")
        );
        Ok(())
    }

    #[test]
    fn logical_edges_handle_unicode_blank_lines_and_inserted_threads() {
        let mut view = View::new("  日本語abc\n\nlast\n".to_owned(), 6, 4);
        view.toggle_source_view();
        view.set_stub_blocks(vec![StubBlock {
            anchor: RowAnchor::Line(1),
            rows: 1,
            stops: vec![0],
            expanded: false,
        }]);
        view.goto_line_end();
        assert_eq!(view.source_position(), (1, 10));
        view.goto_line_start();
        assert_eq!(view.cursor(), Cursor { row: 0, col: 0 });
        view.goto_source_line(2);
        let blank = view.cursor();
        view.goto_line_end();
        view.goto_line_start();
        assert_eq!(view.cursor(), blank);
        view.goto_bottom();
        view.goto_line_start();
        assert_eq!(view.source_position(), (3, 0));
    }

    #[test]
    fn logical_edges_keep_soft_breaks_in_the_same_rendered_row_separate() {
        let mut view = View::new("first line\nsecond line\n".to_owned(), 80, 5);
        view.goto_line_end();
        assert_eq!(view.source_position(), (1, 9));
        view.move_right();
        view.move_right();
        view.goto_line_end();
        assert_eq!(view.source_position(), (2, 10));
        view.goto_line_start();
        assert_eq!(view.source_position(), (2, 0));
    }

    #[test]
    fn references_include_wrapped_url_queries_and_fragments() {
        let url = "https://example.com/search?q=rust&sort=new#results";
        let mut view = View::new(format!("{url}\n"), 12, 8);
        view.toggle_source_view();
        view.move_down(2);
        assert_eq!(view.file_reference().as_deref(), Some(url));
    }
}
