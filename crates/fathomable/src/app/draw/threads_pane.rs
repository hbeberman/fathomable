// @okf-doc: /decisions/0066-one-circle-language.md
//! The threads pane drawn (ADR 0027, ADR 0049, ADR 0066): a rule, a
//! header with the scope and the counts by colour, then a row per file
//! over two rows per thread, and, while the pane has the keys, a key
//! bar along its bottom row in place of the last entry row.
//!
//! A thread's first row is its circle, its place, and the author of
//! its newest message, with the reply count and the age at the right
//! edge; its second row is that message's first line. Both sit in the
//! nest (ADR 0077), under the file row's path. The cursor's entry
//! draws both rows on the active or remembered selection surface,
//! with a cursor bar only while the pane owns navigation. The current
//! file's other rows keep their context tint; a folded file draws its row alone.

use fathomable_core::layout::display_width;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::draw::header::{threads_pane_footer, threads_pane_header};
use crate::app::draw::nest::NEST;
use crate::app::draw::selection::Navigation;
use crate::app::draw::{
    Theme, file_chevron, fit, fit_ellipsis, format_age_short, mark_style, sidebar_divider_style,
};
use crate::app::threads::pane::{PaneEntry, PaneLine, PaneRow, PaneScope, pane_lines};
use crate::app::{App, Focus};

/// Cells a thread's second row is indented, under its place: the
/// leading space, the nest, and the circle with its space.
const SUMMARY_INDENT: usize = 1 + NEST + 2;

/// The pane's rows for a column `width` cells wide and `rows` tall.
pub(super) fn threads_pane_lines<'a>(
    app: &App,
    theme: &Theme,
    width: usize,
    rows: usize,
) -> Vec<Line<'a>> {
    let inner = width.saturating_sub(1);
    let divider_style = sidebar_divider_style(theme);
    let divider = Span::styled("│", divider_style);
    let with_divider = |mut line: Line<'a>| {
        line.spans.push(divider.clone());
        line
    };
    let mut out = Vec::with_capacity(rows);
    out.push(Line::from(vec![
        Span::styled("─".repeat(inner), theme.info),
        Span::styled("┤", divider_style),
    ]));
    let header = threads_pane_header(app);
    let header_row = app.pane_top() + app.tree_rows() + 1;
    let title_hovered = app
        .pointer()
        .is_some_and(|(column, row)| row == header_row && column < header.left_width());
    out.push(with_divider(header.line_with_left_hover(
        theme,
        inner,
        title_hovered,
    )));
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
    let navigation = Navigation::for_pane(app, Focus::ThreadsPane);
    let lines = pane_lines(&entries);
    let scroll = app.threads_pane_scroll(&entries, &lines);
    let now = fathomable_core::clock::now();
    for line in lines.iter().skip(scroll).take(app.threads_pane_body_rows()) {
        let drawn = match *line {
            PaneLine::File(index) => file_line(theme, &entries[index], inner, navigation),
            PaneLine::First(index) => match &entries[index] {
                PaneRow::Thread(entry) => first_line(theme, entry, inner, now, navigation),
                PaneRow::File { .. } => Line::from(""),
            },
            PaneLine::Second(index) => match &entries[index] {
                PaneRow::Thread(entry) => second_line(theme, entry, inner, navigation),
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
        let footer = threads_pane_footer(app);
        let footer_row = app.pane_top() + app.tree_rows() + rows - 1;
        let hovered = app
            .pointer()
            .filter(|(_, row)| *row == footer_row)
            .and_then(|(column, _)| footer.action_at(inner, column));
        out.push(with_divider(
            footer.line_with_action_hover(theme, inner, hovered),
        ));
    }
    out
}

/// The row's style: the sidebar's, the focus tint on the current
/// file's rows, and the shared active or remembered selection.
fn row_style(theme: &Theme, current: bool, selected: bool, navigation: Navigation) -> Style {
    let mut style = theme.sidebar;
    if current {
        style = style.patch(theme.thread_focus);
    }
    style.patch(navigation.selection(selected).style(theme))
}

/// `mark` drawn over the row's background.
fn on(row: Style, mark: Style) -> Style {
    row.bg.map_or(mark, |bg| mark.bg(bg)).patch(Style {
        add_modifier: row.add_modifier,
        ..Style::default()
    })
}

/// A file's row: its path in the directory colour after `▾`, or `▸`
/// when folded (ADR 0076), the thread count at the right edge.
fn file_line<'a>(theme: &Theme, row: &PaneRow, inner: usize, navigation: Navigation) -> Line<'a> {
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
    let style = row_style(theme, *current, *selected, navigation);
    let count = format!("{count} ");
    let name = format!("{} {}", file_chevron(*folded), path.display());
    let name_width = inner.saturating_sub(1 + display_width(&count));
    Line::from(vec![
        navigation.selection(*selected).marker(theme),
        Span::styled(
            fit_ellipsis(&name, name_width),
            on(style, theme.sidebar_dir),
        ),
        Span::styled(count, on(style, theme.info)),
    ])
    .style(style)
}

/// A thread's first row: after the nest, the circle in the state
/// colour, the place dim, the newest author, then `↩n age` at the
/// right edge.
fn first_line<'a>(
    theme: &Theme,
    entry: &PaneEntry,
    inner: usize,
    now: u64,
    navigation: Navigation,
) -> Line<'a> {
    let style = row_style(theme, entry.current(), entry.selected(), navigation);
    let summary = entry.summary_facts();
    let place = format!(" {} ", summary.location());
    let age = format_age_short(summary.modified(), now);
    let tail = if summary.replies() > 0 {
        format!("↩{} {age}", summary.replies())
    } else {
        age
    };
    let lead = 1 + NEST + display_width(entry.words().glyph()) + display_width(&place);
    // The author takes what is left before the tail: its full name when it
    // fits, cut with `…` beyond that.
    let free = inner.saturating_sub(lead + display_width(&tail) + 1);
    let full = summary.author();
    let short = summary.author();
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
    let branch = summary
        .context()
        .map(|note| format!(" {note}"))
        .filter(|branch| {
            lead + display_width(&author) + display_width(branch) + display_width(&tail) < inner
        })
        .unwrap_or_default();
    let pad = inner.saturating_sub(
        lead + display_width(&author) + display_width(&branch) + display_width(&tail),
    );
    Line::from(vec![
        navigation.selection(entry.selected()).marker(theme),
        Span::styled(" ".repeat(NEST), style),
        Span::styled(
            entry.words().glyph(),
            on(style, mark_style(theme, entry.kind())),
        ),
        Span::styled(place, on(style, theme.info)),
        Span::styled(
            author,
            on(
                style,
                if summary.author_is_user() {
                    theme.thread_user
                } else {
                    theme.thread_agent
                },
            ),
        ),
        Span::styled(branch, on(style, theme.info)),
        Span::styled(" ".repeat(pad), style),
        Span::styled(tail, on(style, theme.info)),
    ])
    .style(style)
}

/// A thread's second row: the newest message's first line, indented
/// under the place and cut with `…`.
fn second_line<'a>(
    theme: &Theme,
    entry: &PaneEntry,
    inner: usize,
    navigation: Navigation,
) -> Line<'a> {
    let style = row_style(theme, entry.current(), entry.selected(), navigation);
    let text_style = if entry.words().is_resolved() {
        on(style, theme.info)
    } else {
        style
    };
    Line::from(vec![
        navigation.selection(entry.selected()).marker(theme),
        Span::styled(
            " ".repeat(SUMMARY_INDENT.min(inner).saturating_sub(1)),
            style,
        ),
        Span::styled(
            fit_ellipsis(
                entry.summary_facts().preview(),
                inner.saturating_sub(SUMMARY_INDENT),
            ),
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

    #[test]
    fn narrow_author_rows_preserve_parentheses_in_names() -> anyhow::Result<()> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        let theme = crate::app::draw::Theme::from_core(&core);
        for (case, author, name, full) in [
            ("user", Author::User, "Henry (work)", "Henry (work)"),
            (
                "agent",
                Author::agent("Bot (work)"),
                "Bot (work)",
                "Bot (work)",
            ),
        ] {
            let dir = testing::workspace(&format!("pane-author-{case}"), testing::README)?;
            let mut app = source_app(&dir)?;
            app.user.name = "Henry (work)".to_owned();
            app.view_mut().goto_source_line(3);
            app.start_new_comment();
            app.compose_insert("question");
            app.compose_submit();
            let id = app.file_threads()[0].clone();
            if author.is_user() {
                app.thread_reply();
                app.compose_insert("answer");
                app.compose_submit();
            } else {
                let reply = app.handle_request(Request::ThreadReply {
                    thread: id,
                    author,
                    caller: "test:viewer".to_owned(),
                    body: "answer".to_owned(),
                    resolve: false,
                    lines: None,
                    idempotency_key: None,
                });
                assert!(!matches!(reply, Response::Error(_)), "{reply:?}");
            }
            let entries = app.threads_pane_entries();
            let entry = &entries[0];
            assert_eq!(entry.author(), full);
            assert_eq!(entry.author_name(), name);
            for (free, expected) in [
                (full.len(), full),
                (name.len(), name),
                (
                    8,
                    if case == "user" {
                        "Henry (…"
                    } else {
                        "Bot (wo…"
                    },
                ),
            ] {
                // L3, the nest, and the reply/age tail reserve fifteen cells.
                let inner = free + 15;
                let line = super::first_line(
                    &theme,
                    entry,
                    inner,
                    entry.updated(),
                    super::Navigation::Inactive,
                );
                assert_eq!(line.spans[4].content, expected, "{case} at {free}");
                assert_eq!(line.width(), inner);
                assert!(line.to_string().ends_with("↩1 now"));
            }
        }
        Ok(())
    }

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

    /// A thread's first row names the newest author; the second row cuts
    /// the message with `…`; a proposed thread draws `◐`
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
            author: Author::agent("reviewer"),
            caller: "test:viewer".to_owned(),
            body: "done, I think, though the empty string still wants a test of its own".to_owned(),
            resolve: true,
            lines: None,
            idempotency_key: None,
        });
        assert!(!matches!(reply, Response::Error(_)), "{reply:?}");
        app.open(Path::new("README.md"));
        app.view_mut().goto_source_line(3);

        let column = sidebar_column(&app, 100)?;
        let top = app.tree_rows();
        let first = column[top + 2].trim_end_matches('│').to_owned();
        assert!(
            first.starts_with("   ◐ L3 reviewer "),
            "the circle in the nest, under the path (ADR 0077): {first:?}"
        );
        assert!(first.ends_with("↩1 now"), "{first:?}");
        assert!(column[top + 3].ends_with("…│"), "{:?}", column[top + 3]);
        assert!(
            column[top + 3].starts_with("     done, I think"),
            "the summary under the place, in the nest (ADR 0077): {:?}",
            column[top + 3]
        );
        assert!(
            column[top + 1].contains("◐ 1"),
            "one proposed; narrow headers drop all words together: {:?}",
            column[top + 1]
        );

        // Narrow: the author still fits before the tail.
        app.resize(72, 30);
        let column = sidebar_column(&app, 72)?;
        let first = column[top + 2].trim_end_matches('│').to_owned();
        assert!(first.contains("◐ L3 reviewer "), "{first:?}");
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
        assert!(focused[last].contains("reply c"), "{:?}", focused[last]);
        assert_eq!(
            focused[top + 2].chars().skip(1).collect::<String>(),
            column[top + 2].chars().skip(1).collect::<String>(),
            "rows above do not move when the cursor bar appears"
        );
        assert_eq!(focused[last - 1], column[last - 1]);

        app.leave_threads_pane();
        let back = sidebar_column(&app, 100)?;
        assert_eq!(back[last], column[last], "the entry row is back");
        Ok(())
    }
}
