// @okf-doc: /decisions/0066-one-circle-language.md
//! The threads pane drawn (ADR 0027, ADR 0049, ADR 0066): a rule, a
//! header with the scope and the counts by colour, then a row per file
//! over two rows per thread, and, while the pane has the keys, a key
//! bar along its bottom row in place of the last entry row.
//!
//! A thread's first row is its circle, its place, and the author of
//! its newest message, with the reply count and the age at the right
//! edge; its second row is that message's first line. The cursor's
//! entry draws both rows on the selected surface, the current file's
//! rows on the focus tint, and a folded file its row alone.

use fathomable_core::layout::display_width;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::draw::header::{threads_pane_footer, threads_pane_header};
use crate::app::draw::{Theme, fit, fit_ellipsis, format_age_short, mark_style};
use crate::app::threads::pane::{PaneEntry, PaneLine, PaneRow, PaneScope, pane_lines};
use crate::app::{App, Focus};

/// Cells a thread's second row is indented, under its place.
const SUMMARY_INDENT: usize = 3;

/// The pane's rows for a column `width` cells wide and `rows` tall.
pub(super) fn threads_pane_lines<'a>(
    app: &App,
    theme: &Theme,
    width: usize,
    rows: usize,
) -> Vec<Line<'a>> {
    let inner = width.saturating_sub(1);
    let divider = Span::styled("│", theme.marker);
    let with_divider = |mut line: Line<'a>| {
        line.spans.push(divider.clone());
        line
    };
    let mut out = Vec::with_capacity(rows);
    out.push(with_divider(Line::from(Span::styled(
        "─".repeat(inner),
        theme.info,
    ))));
    out.push(with_divider(threads_pane_header(app).line(theme, inner)));
    let entries = app.threads_pane_rows();
    if entries.is_empty() {
        let empty = match app.sidebar_scope() {
            PaneScope::File => " no threads in this file",
            PaneScope::Workspace => " no threads in the workspace",
        };
        out.push(with_divider(Line::from(Span::styled(
            fit(empty, inner),
            theme.info,
        ))));
    }
    let focused = app.focus() == Focus::ThreadsPane;
    let lines = pane_lines(&entries);
    let scroll = app.threads_pane_scroll(&entries, &lines);
    let now = fathomable_core::clock::now();
    for line in lines.iter().skip(scroll).take(app.threads_pane_body_rows()) {
        let drawn = match *line {
            PaneLine::File(index) => file_line(theme, &entries[index], inner),
            PaneLine::First(index) => match &entries[index] {
                PaneRow::Thread(entry) => first_line(theme, entry, inner, now),
                PaneRow::File { .. } => Line::from(""),
            },
            PaneLine::Second(index) => match &entries[index] {
                PaneRow::Thread(entry) => second_line(theme, entry, inner),
                PaneRow::File { .. } => Line::from(""),
            },
        };
        out.push(with_divider(drawn));
    }
    let body_end = rows.saturating_sub(usize::from(focused));
    while out.len() < body_end {
        out.push(with_divider(Line::from(Span::styled(
            " ".repeat(inner),
            theme.sidebar,
        ))));
    }
    out.truncate(body_end);
    if focused && rows > 0 {
        out.push(with_divider(threads_pane_footer(app).line(theme, inner)));
    }
    out
}

/// The row's style: the sidebar's, the focus tint on the current
/// file's rows, the selected surface in bold on the cursor's entry.
fn row_style(theme: &Theme, current: bool, selected: bool) -> Style {
    let mut style = theme.sidebar;
    if current {
        style = style.patch(theme.thread_focus);
    }
    if selected {
        style = style
            .patch(theme.picker_selected)
            .add_modifier(Modifier::BOLD);
    }
    style
}

/// `mark` drawn over the row's background.
fn on(row: Style, mark: Style) -> Style {
    row.bg.map_or(mark, |bg| mark.bg(bg)).patch(Style {
        add_modifier: row.add_modifier,
        ..Style::default()
    })
}

/// A file's row: its path in the directory colour, `▸` before a folded
/// one, the thread count at the right edge.
fn file_line<'a>(theme: &Theme, row: &PaneRow, inner: usize) -> Line<'a> {
    let PaneRow::File {
        path,
        count,
        folded,
        current,
        selected,
    } = row
    else {
        return Line::from("");
    };
    let style = row_style(theme, *current, *selected);
    let count = format!("{count} ");
    let name = format!(" {}{}", if *folded { "▸ " } else { "" }, path.display());
    let name_width = inner.saturating_sub(display_width(&count));
    Line::from(vec![
        Span::styled(
            fit_ellipsis(&name, name_width),
            on(style, theme.sidebar_dir),
        ),
        Span::styled(count, on(style, theme.info)),
    ])
    .style(style)
}

/// A thread's first row: the circle in the state colour, the place dim,
/// the newest author, then `↩n age` at the right edge.
fn first_line<'a>(theme: &Theme, entry: &PaneEntry, inner: usize, now: u64) -> Line<'a> {
    let style = row_style(theme, entry.current(), entry.selected());
    let place = format!(" {} ", entry.place());
    let age = format_age_short(entry.updated(), now);
    let tail = if entry.replies() > 0 {
        format!("↩{} {age}", entry.replies())
    } else {
        age
    };
    let lead = 1 + display_width(entry.words().glyph()) + display_width(&place);
    // The author takes what is left before the tail: `name (role)` when
    // it fits, `name` when it does not, cut with `…` beyond that.
    let free = inner.saturating_sub(lead + display_width(&tail) + 1);
    let full = entry.author();
    let short = full.split(" (").next().unwrap_or(full);
    let author = if display_width(full) <= free {
        full.to_owned()
    } else if display_width(short) <= free {
        short.to_owned()
    } else {
        fit_ellipsis(short, free).trim_end().to_owned()
    };
    // The branch of the worktree showing it (ADR 0070), or the commit a
    // past thread was resolved at (ADR 0072), dim, after the author;
    // dropped before the author is cut.
    let branch = entry
        .note()
        .map(|note| format!(" {note}"))
        .filter(|branch| {
            lead + display_width(&author) + display_width(branch) + display_width(&tail) < inner
        })
        .unwrap_or_default();
    let pad = inner.saturating_sub(
        lead + display_width(&author) + display_width(&branch) + display_width(&tail),
    );
    Line::from(vec![
        Span::styled(" ", style),
        Span::styled(
            entry.words().glyph(),
            on(style, mark_style(theme, entry.kind())),
        ),
        Span::styled(place, on(style, theme.info)),
        Span::styled(author, on(style, theme.popup_key)),
        Span::styled(branch, on(style, theme.info)),
        Span::styled(" ".repeat(pad), style),
        Span::styled(tail, on(style, theme.info)),
    ])
    .style(style)
}

/// A thread's second row: the newest message's first line, indented
/// under the place and cut with `…`.
fn second_line<'a>(theme: &Theme, entry: &PaneEntry, inner: usize) -> Line<'a> {
    let style = row_style(theme, entry.current(), entry.selected());
    let text_style = if entry.words().is_resolved() {
        on(style, theme.info)
    } else {
        style
    };
    Line::from(vec![
        Span::styled(" ".repeat(SUMMARY_INDENT.min(inner)), style),
        Span::styled(
            fit_ellipsis(entry.summary(), inner.saturating_sub(SUMMARY_INDENT)),
            text_style,
        ),
    ])
    .style(style)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use fathomable_core::annotations::Author;
    use fathomable_core::session::{Request, Response};

    use crate::app::testing::{self, source_app};
    use crate::app::{App, Focus};

    fn sidebar_column(app: &App, width: u16) -> anyhow::Result<Vec<String>> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::draw::Theme::from_core(&core);
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 30))?;
        terminal.draw(|frame| crate::app::draw::draw(frame, app, &theme))?;
        let buffer = terminal.backend().buffer().clone();
        Ok((0..buffer.area.height)
            .map(|y| {
                (0..u16::try_from(app.sidebar_width()).unwrap_or(0))
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect())
    }

    /// A thread's first row names the newest author with the role when
    /// the column has room and without it when it does not; the second
    /// row cuts the message with `…`; a proposed thread draws `◐`
    /// (ADR 0066).
    #[test]
    fn rows_name_the_author_and_cut_the_message_with_an_ellipsis() -> anyhow::Result<()> {
        let dir = testing::workspace("pane-draw-rows", testing::README)?;
        fs::create_dir_all(dir.0.join("ws/docs"))?;
        let mut app = source_app(&dir)?;
        app.show_threads_pane();
        app.view_mut().goto_source_line(3);
        app.start_new_comment();
        app.compose_insert("a question long enough to be cut off at the pane's edge");
        app.compose_submit();
        let id = app.file_threads()[0].clone();
        let reply = app.handle_request(Request::ThreadReply {
            thread: id.clone(),
            author: Author::agent("reviewer").subscribed("s-1", "coder"),
            body: "done, I think, though the empty string still wants a test of its own".to_owned(),
            resolve: true,
            lines: None,
        });
        assert!(!matches!(reply, Response::Error(_)), "{reply:?}");
        app.open(Path::new("README.md"));
        app.view_mut().goto_source_line(3);

        let column = sidebar_column(&app, 100)?;
        let top = app.tree_rows();
        let first = column[top + 2].trim_end_matches('│').to_owned();
        assert!(first.contains("◐ L3 reviewer (coder)"), "{first:?}");
        assert!(first.ends_with("↩1 now"), "{first:?}");
        assert!(column[top + 3].ends_with("…│"), "{:?}", column[top + 3]);
        assert!(
            column[top + 3].starts_with("   done, I think"),
            "{:?}",
            column[top + 3]
        );
        assert!(
            column[top + 1].contains("◐ 1 resolve?"),
            "one proposed, with its word (ADR 0075): {:?}",
            column[top + 1]
        );

        // Narrow: the role goes before the tail does.
        app.resize(66, 30);
        let column = sidebar_column(&app, 66)?;
        let first = column[top + 2].trim_end_matches('│').to_owned();
        assert!(first.contains("◐ L3 reviewer "), "{first:?}");
        assert!(!first.contains("(coder)"), "{first:?}");
        assert!(first.ends_with("↩1 now"), "{first:?}");
        Ok(())
    }

    /// The key bar replaces the bottom row while the pane has the keys
    /// and gives it back when the keys leave; the rows above stay put
    /// (ADR 0066).
    #[test]
    fn the_key_bar_takes_the_bottom_row_only_with_the_keys() -> anyhow::Result<()> {
        let dir = testing::workspace("pane-draw-bar", testing::README)?;
        let mut app = source_app(&dir)?;
        app.show_tree();
        app.show_threads_pane();
        app.focus_pane(Focus::View);
        for line in [2, 3, 5] {
            app.view_mut().goto_source_line(line);
            app.start_new_comment();
            app.compose_insert("one");
            app.compose_submit();
        }
        app.view_mut().goto_source_line(2);
        let column = sidebar_column(&app, 100)?;
        let top = app.tree_rows();
        let last = app.pane_rows() - 1;
        assert!(
            column[last].contains("one"),
            "an entry row: {:?}",
            column[last]
        );
        assert_eq!(app.focus(), Focus::View);

        app.focus_threads_pane();
        let focused = sidebar_column(&app, 100)?;
        assert!(focused[last].contains("s scope"), "{:?}", focused[last]);
        assert_eq!(focused[top + 2], column[top + 2], "rows above do not move");
        assert_eq!(focused[last - 1], column[last - 1]);

        app.leave_threads_pane();
        let back = sidebar_column(&app, 100)?;
        assert_eq!(back[last], column[last], "the entry row is back");
        Ok(())
    }
}
