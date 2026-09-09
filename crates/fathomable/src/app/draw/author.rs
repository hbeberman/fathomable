// @okf-doc: /decisions/0071-author-stripes.md
//! Who wrote a message, drawn (ADR 0071): the stripe under its rows and
//! the colour of its name come from the author's kind, `thread.user` or
//! `thread.agent`; the thread cursor is a bar in `thread.cursor` down
//! the left edge of its message's rows, with the name in bold.
//!
//! The expanded thread in the file and the review list both draw
//! through these, so a message reads the same on either surface.

use fathomable_core::annotations::Author;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::app::draw::Theme;

/// Cells an expanded thread's rows are indented from the global gutter
/// in the file: the cursor bar or a space, then a space.
pub(crate) const THREAD_GUTTER: usize = 2;

/// The glyph of the cursor bar.
pub(crate) const CURSOR_BAR: &str = "▎";

/// The stripe under every row of a message by `author`: the kind's
/// background alone, so the text keeps its own colours.
pub(crate) fn row_style(theme: &Theme, author: &Author) -> Style {
    let kind = if author.is_user() {
        theme.thread_user
    } else {
        theme.thread_agent
    };
    kind.bg
        .map_or_else(Style::default, |bg| Style::default().bg(bg))
}

/// The author's name on its row: the kind's foreground, bold when the
/// message is the cursor's.
pub(crate) fn name_style(theme: &Theme, author: &Author, marked: bool) -> Style {
    let kind = if author.is_user() {
        theme.thread_user
    } else {
        theme.thread_agent
    };
    let style = kind
        .fg
        .map_or_else(|| theme.text, |fg| Style::default().fg(fg));
    if marked {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

/// The first cell of a row: the cursor bar when the row belongs to the
/// cursor's message or its thread's header, a space otherwise.
pub(crate) fn cursor_cell(theme: &Theme, marked: bool) -> Span<'static> {
    if marked {
        Span::styled(CURSOR_BAR, theme.thread_cursor)
    } else {
        Span::raw(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> anyhow::Result<Theme> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        Ok(Theme::from_core(&core))
    }

    #[test]
    fn the_user_and_an_agent_take_their_own_stripe_and_name_colour() -> anyhow::Result<()> {
        let theme = theme()?;
        let agent = Author::agent("coder");
        assert_eq!(row_style(&theme, &Author::User).bg, theme.thread_user.bg);
        assert_eq!(row_style(&theme, &agent).bg, theme.thread_agent.bg);
        assert_ne!(theme.thread_user.bg, theme.thread_agent.bg);
        assert!(
            row_style(&theme, &Author::User).fg.is_none(),
            "the stripe is a background"
        );
        assert_eq!(
            name_style(&theme, &Author::User, false).fg,
            theme.thread_user.fg
        );
        assert_eq!(name_style(&theme, &agent, false).fg, theme.thread_agent.fg);
        assert!(
            name_style(&theme, &agent, true)
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(cursor_cell(&theme, true).content, CURSOR_BAR);
        assert_eq!(cursor_cell(&theme, false).content, " ");
        Ok(())
    }

    /// ADR 0071: in both built-in themes the state colours are the
    /// author colours, open the user's and waiting the agents', so the
    /// circle and the stripe never disagree; resolved stays grey.
    #[test]
    fn the_state_colours_are_the_author_colours_in_the_built_in_themes() -> anyhow::Result<()> {
        for name in ["default-dark", "default-light"] {
            let core = fathomable_core::theme::Theme::resolve(name, |_| Ok(None))?;
            let theme = Theme::from_core(&core);
            assert_eq!(theme.thread_open.fg, theme.thread_user.fg, "{name}");
            assert_eq!(theme.thread_waiting.fg, theme.thread_agent.fg, "{name}");
            assert_ne!(theme.thread_resolved.fg, theme.thread_user.fg, "{name}");
            assert_ne!(theme.thread_resolved.fg, theme.thread_agent.fg, "{name}");
        }
        Ok(())
    }
}
