// @okf-doc: /decisions/0074-the-bracket-marks-the-focused-thread.md
//! The note cell as the text draws it (ADR 0074): the gutter's bracket
//! or circle in the thread's state colour over the row's surface, and on
//! the rows of the thread the cursor is on, on `thread.bracket`, so the
//! bracket alone says which lines the focused thread is about.

use ratatui::style::Style;
use ratatui::text::Span;

use crate::app::App;
use crate::app::draw::{Theme, mark_style};

/// The note cell of rendered row `row` over `surface`: the glyph
/// `note_on_row` gives in its state colour, or a blank. The cell sits on
/// `thread.bracket` when it holds a glyph and the thread cursor's thread
/// covers the row (ADR 0074).
pub(super) fn note_cell<'a>(app: &App, theme: &Theme, row: usize, surface: Style) -> Span<'a> {
    match app.note_on_row(row) {
        None => Span::styled(" ", surface),
        Some((glyph, kind)) => {
            let mut style = mark_style(theme, kind).patch(surface);
            if app.open_thread_on_row(row) {
                style = style.patch(theme.thread_bracket);
            }
            Span::styled(glyph, style)
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;

    use crate::app::App;
    use crate::app::testing::{self, buffer, source_app};

    fn row_of(buffer: &Buffer, text: &str) -> anyhow::Result<u16> {
        (0..buffer.area.height)
            .find(|&y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains(text)
            })
            .ok_or_else(|| anyhow::anyhow!("no row holds {text:?}"))
    }

    fn comment_on(app: &mut App, first: usize, last: usize, text: &str) {
        app.view_mut().goto_source_line(first);
        app.view_mut().select_lines();
        app.view_mut().move_down(last - first);
        app.start_new_comment();
        app.compose_insert(text);
        app.compose_submit();
    }

    /// The focused thread's bracket cells sit on `thread.bracket`, its
    /// text carries no tint, and another thread's bracket stays dark
    /// (ADR 0074).
    #[test]
    fn the_focused_threads_bracket_lights_up_and_its_lines_do_not() -> anyhow::Result<()> {
        let dir = testing::workspace("bracket-lights", testing::README)?;
        let mut app = source_app(&dir)?;
        comment_on(&mut app, 7, 7, "later");
        comment_on(&mut app, 3, 4, "mine");
        app.view_mut().goto_source_line(3);
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::draw::Theme::from_core(&core);
        let lit = theme.thread_bracket.bg.unwrap_or_default();
        let screen = buffer(&app)?;
        let text_x = screen.area.width - u16::try_from(app.view().layout().width())?;
        let note_x = text_x - 4;
        let alpha = row_of(&screen, "alpha")?;
        let beta = row_of(&screen, "beta")?;
        let gamma = row_of(&screen, "gamma")?;
        let one = row_of(&screen, "- one")?;
        assert_eq!(screen[(note_x, alpha)].symbol(), "╭");
        assert_eq!(screen[(note_x, beta)].symbol(), "╰");
        assert_eq!(
            screen[(note_x, alpha)].bg,
            lit,
            "the first row's corner is lit"
        );
        assert_eq!(
            screen[(note_x, beta)].bg,
            lit,
            "the last row's corner is lit"
        );
        assert_eq!(
            screen[(text_x, alpha)].bg,
            screen[(text_x, gamma)].bg,
            "the thread's text sits on the same ground as any line"
        );
        assert_eq!(
            screen[(note_x + 1, alpha)].bg,
            screen[(note_x + 1, gamma)].bg,
            "the number cell is not lit"
        );
        assert_ne!(
            screen[(note_x, one)].bg,
            lit,
            "the other thread's circle is not lit"
        );
        assert_ne!(
            screen[(note_x, one)].symbol(),
            " ",
            "the other thread is bracketed"
        );
        Ok(())
    }
}
