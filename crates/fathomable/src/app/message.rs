// @okf-doc: /decisions/0037-markdown-in-threads.md
//! Thread-pane messages rendered as Markdown (ADR 0037).
//!
//! A comment or reply body goes through the same renderer as a Markdown
//! file, wrapped to the pane's width less the message indent, with fenced
//! code coloured by its language. The row count the pane scrolls by is
//! taken from the same rendering, so the two cannot disagree.

use fathomable_core::annotations::Thread;
use fathomable_core::highlight::Highlighter;
use fathomable_core::layout::{Layout, display_width};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

use super::ui::{SNIPPET_ROWS, Theme, face_style, fit, format_age};

/// Cells a message body sits in from the pane's left edge.
const MESSAGE_INDENT: usize = 3;

/// One message of a thread: the comment or a reply.
struct Message<'a> {
    author: &'a str,
    created: u64,
    body: &'a str,
    badge: Option<&'a str>,
}

/// The pane's body for `thread`, whose snippet starts at source line
/// `first`: the quoted snippet, a blank, the comment, each reply after a
/// blank, and the END row of ADR 0034.
pub(super) fn thread_body_lines<'a>(
    theme: &Theme,
    highlighter: &Highlighter,
    thread: &Thread,
    first: usize,
    now: u64,
    width: usize,
    selected: usize,
) -> Vec<Line<'a>> {
    let inner = width.saturating_sub(2);
    let mut body: Vec<Line<'_>> = Vec::new();
    let snippet: Vec<&str> = thread.snippet().lines().collect();
    let number_width = (first + snippet.len()).to_string().len();
    for (offset, line) in snippet.iter().take(SNIPPET_ROWS).enumerate() {
        body.push(Line::from(Span::styled(
            fit(
                &format!(" {:>number_width$} │ {line}", first + offset),
                width,
            ),
            theme.info,
        )));
    }
    if snippet.len() > SNIPPET_ROWS {
        body.push(Line::from(Span::styled(
            format!(" {:>number_width$} │ …", ""),
            theme.info,
        )));
    }
    body.push(Line::from(""));
    let comment = Message {
        author: "user",
        created: thread.created(),
        body: thread.comment(),
        badge: None,
    };
    body.extend(message_lines(
        theme,
        highlighter,
        &comment,
        now,
        inner,
        selected == 0,
    ));
    for (index, reply) in thread.replies().iter().enumerate() {
        body.push(Line::from(""));
        let message = Message {
            author: reply.author().name(),
            created: reply.created(),
            body: reply.body(),
            badge: reply.proposes_resolution().then_some("proposes resolving"),
        };
        body.extend(message_lines(
            theme,
            highlighter,
            &message,
            now,
            inner,
            selected == index + 1,
        ));
    }
    body.push(Line::from(Span::styled(
        " ─── END ───",
        theme.info.add_modifier(Modifier::DIM),
    )));
    body
}

/// A message: author, age and an optional badge on one row, the body
/// rendered as Markdown and indented beneath it.
fn message_lines<'a>(
    theme: &Theme,
    highlighter: &Highlighter,
    message: &Message<'_>,
    now: u64,
    width: usize,
    selected: bool,
) -> Vec<Line<'a>> {
    let mut header = vec![
        Span::styled(format!(" {}", message.author), theme.popup_key),
        Span::styled(
            format!("  {}", format_age(message.created, now)),
            theme.info,
        ),
    ];
    if let Some(badge) = message.badge {
        header.push(Span::styled(format!("  [{badge}]"), theme.thread_open));
    }
    let mut out = vec![message_line(theme, header, width, selected)];
    let indent = " ".repeat(MESSAGE_INDENT);
    for line in body_layout(message.body, width, highlighter).lines() {
        let mut spans = vec![Span::raw(indent.clone())];
        spans.extend(
            line.spans()
                .iter()
                .map(|span| Span::styled(span.text().to_owned(), face_style(theme, span.style()))),
        );
        out.push(message_line(theme, spans, width, selected));
    }
    out
}

fn message_line<'a>(
    theme: &Theme,
    mut spans: Vec<Span<'a>>,
    width: usize,
    selected: bool,
) -> Line<'a> {
    if selected {
        let used = spans
            .iter()
            .map(|span| display_width(&span.content))
            .sum::<usize>();
        spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
        Line::from(spans).style(theme.picker_selected)
    } else {
        Line::from(spans)
    }
}

/// Rows the pane's body takes for `thread` at `width`: snippet, blank,
/// comment, each reply after a blank, and the END row.
pub(super) fn thread_body_rows(thread: &Thread, width: usize, highlighter: &Highlighter) -> usize {
    let inner = width.saturating_sub(2);
    let snippet = thread.snippet().lines().count();
    let snippet_rows = snippet.min(SNIPPET_ROWS) + usize::from(snippet > SNIPPET_ROWS);
    let message_rows = |body: &str| 1 + body_layout(body, inner, highlighter).lines().len();
    let replies: usize = thread
        .replies()
        .iter()
        .map(|reply| 1 + message_rows(reply.body()))
        .sum();
    snippet_rows + 1 + message_rows(thread.comment()) + replies + 1
}

/// The body rows occupied by message `selected`, zero for the comment.
pub(super) fn thread_message_range(
    thread: &Thread,
    selected: usize,
    width: usize,
    highlighter: &Highlighter,
) -> Option<std::ops::Range<usize>> {
    let inner = width.saturating_sub(2);
    let snippet = thread.snippet().lines().count();
    let mut start = snippet.min(SNIPPET_ROWS) + usize::from(snippet > SNIPPET_ROWS) + 1;
    let rows = |body: &str| 1 + body_layout(body, inner, highlighter).lines().len();
    let comment_rows = rows(thread.comment());
    if selected == 0 {
        return Some(start..start + comment_rows);
    }
    start += comment_rows;
    for (index, reply) in thread.replies().iter().enumerate() {
        start += 1;
        let reply_rows = rows(reply.body());
        if selected == index + 1 {
            return Some(start..start + reply_rows);
        }
        start += reply_rows;
    }
    None
}

/// The body laid out as Markdown in the cells left of `width` after the
/// indent, a newline kept as a line break, fenced code coloured by
/// `highlighter`.
fn body_layout(body: &str, width: usize, highlighter: &Highlighter) -> Layout {
    Layout::render_message(
        body,
        width.saturating_sub(MESSAGE_INDENT).max(1),
        highlighter,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> anyhow::Result<Theme> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        Ok(Theme::from_core(&core))
    }

    fn texts(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    fn render(body: &str, badge: Option<&str>, width: usize) -> anyhow::Result<Vec<Line<'static>>> {
        let message = Message {
            author: "Copilot",
            created: 0,
            body,
            badge,
        };
        Ok(message_lines(
            &theme()?,
            &Highlighter::plain(),
            &message,
            0,
            width,
            false,
        ))
    }

    #[test]
    fn plain_sentence_renders_as_itself_under_the_header() -> anyhow::Result<()> {
        let lines = render("please check this", None, 40)?;
        assert_eq!(
            texts(&lines),
            [" Copilot  just now", "   please check this"]
        );
        Ok(())
    }

    #[test]
    fn a_newline_stays_a_line_break() -> anyhow::Result<()> {
        let lines = render("first\nsecond", None, 40)?;
        assert_eq!(texts(&lines)[1..], ["   first", "   second"]);
        Ok(())
    }

    #[test]
    fn markdown_blocks_render_and_wrap_to_the_indented_width() -> anyhow::Result<()> {
        let body = "Two **points**:\n\n- first\n- second `x`\n\n```rust\nfn a() {}\n```\n";
        let lines = render(body, Some("proposes resolving"), 30)?;
        let rows = texts(&lines);
        assert_eq!(rows[0], " Copilot  just now  [proposes resolving]");
        assert!(rows.iter().any(|t| t == "   Two points:"), "{rows:?}");
        assert!(rows.iter().any(|t| t.contains("• first")), "{rows:?}");
        assert!(rows.iter().any(|t| t.contains("fn a() {}")), "{rows:?}");
        let bold = lines[1].spans.iter().find(|s| s.content == "points");
        assert!(
            bold.is_some_and(|s| s.style.add_modifier.contains(Modifier::BOLD)),
            "strong span"
        );
        let long = "word ".repeat(20);
        let wrapped = render(&long, None, 30)?;
        assert!(wrapped.len() > 3, "wrapped: {}", wrapped.len());
        assert!(texts(&wrapped).iter().all(|t| t.chars().count() <= 30));
        Ok(())
    }

    #[test]
    fn a_selected_message_fills_each_row_with_the_selection_style() -> anyhow::Result<()> {
        let theme = theme()?;
        let message = Message {
            author: "user",
            created: 0,
            body: "selected",
            badge: None,
        };
        let lines = message_lines(&theme, &Highlighter::plain(), &message, 0, 24, true);
        assert!(lines.iter().all(|line| line.style == theme.picker_selected));
        assert!(texts(&lines).iter().all(|line| display_width(line) == 24));
        Ok(())
    }
}
