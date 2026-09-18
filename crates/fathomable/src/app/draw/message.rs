// @okf-doc: /decisions/0037-markdown-in-threads.md
//! Thread messages rendered as Markdown (ADR 0037), in the rows of an
//! expanded thread (ADR 0049).
//!
//! A comment or reply body goes through the same renderer as a Markdown
//! file, wrapped to the text width less the message indent, with fenced
//! code coloured by its language. File and Reviews surfaces share retained
//! body layouts; inline row counts and drawing use those same layouts, so
//! navigation and drawing do not parse or highlight a body again.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

#[cfg(test)]
use std::cell::Cell;

use fathomable_core::annotations::{Author, Thread, ThreadId};
use fathomable_core::highlight::Highlighter;
use fathomable_core::layout::{Layout, display_width};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::draw::author::{THREAD_GUTTER, cursor_cell, name_style, row_style};
use crate::app::draw::{Theme, face_style, format_age};
use crate::app::threads::author_label;

/// Cells a message body sits in from the block's left edge: the
/// thread's own gutter (ADR 0071) and two more.
pub(crate) const MESSAGE_INDENT: usize = THREAD_GUTTER + 2;
const CACHED_WIDTHS_PER_THREAD: usize = 2;

/// One message of a thread: the comment or a reply.
struct Message<'a> {
    author: &'a Author,
    name: &'a str,
    created: u64,
    badge: Option<&'a str>,
}

/// Rendered bodies shared by every thread surface at one effective width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageLayouts {
    revision: u64,
    width: usize,
    bodies: Vec<Layout>,
}

impl MessageLayouts {
    fn new(thread: &Thread, width: usize, highlighter: &Highlighter) -> Self {
        let bodies = std::iter::once(thread.comment())
            .chain(
                thread
                    .replies()
                    .iter()
                    .map(fathomable_core::annotations::Reply::body),
            )
            .map(|body| Layout::render_message(body, width.max(1), highlighter))
            .collect();
        Self {
            revision: thread.revision(),
            width,
            bodies,
        }
    }

    fn matches(&self, thread: &Thread, width: usize) -> bool {
        self.revision == thread.revision() && self.width == width
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "construction stores exactly one body for every message index"
    )]
    pub(crate) fn body(&self, message: usize) -> &Layout {
        &self.bodies[message]
    }
}

/// Retains the two most recent message widths for every rendered thread.
#[derive(Debug, Default)]
pub(crate) struct MessageLayoutCache {
    entries: RefCell<HashMap<ThreadId, VecDeque<Arc<MessageLayouts>>>>,
    #[cfg(test)]
    renders: Cell<usize>,
}

impl MessageLayoutCache {
    /// Reuse a matching thread revision and effective body width.
    pub(crate) fn layout(
        &self,
        thread: &Thread,
        width: usize,
        highlighter: &Highlighter,
    ) -> Arc<MessageLayouts> {
        let mut entries = self.entries.borrow_mut();
        let variants = entries.entry(thread.id().clone()).or_default();
        variants.retain(|layout| layout.revision == thread.revision());
        if let Some(at) = variants
            .iter()
            .position(|layout| layout.matches(thread, width))
            && let Some(layout) = variants.remove(at)
        {
            variants.push_back(Arc::clone(&layout));
            return layout;
        }

        let layout = Arc::new(MessageLayouts::new(thread, width, highlighter));
        if variants.len() >= CACHED_WIDTHS_PER_THREAD {
            variants.pop_front();
        }
        variants.push_back(Arc::clone(&layout));
        #[cfg(test)]
        self.renders.set(self.renders.get() + 1);
        layout
    }

    #[cfg(test)]
    pub(crate) fn renders(&self) -> usize {
        self.renders.get()
    }
}

/// Placement metadata for one expanded thread at its outer width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExpandedLayout {
    width: usize,
    bodies: Arc<MessageLayouts>,
    rows: usize,
    stops: Vec<usize>,
}

impl ExpandedLayout {
    /// Derive row placement from retained message bodies.
    pub(crate) fn new(width: usize, bodies: Arc<MessageLayouts>) -> Self {
        let mut stops = Vec::with_capacity(bodies.bodies.len());
        let mut rows = 0;
        for body in &bodies.bodies {
            stops.push(rows);
            rows += 1 + body.lines().len();
        }
        Self {
            width,
            bodies,
            rows,
            stops,
        }
    }

    /// Whether this layout still describes `thread` at `width`.
    pub(crate) fn matches(&self, thread: &Thread, width: usize) -> bool {
        self.width == width
            && self
                .bodies
                .matches(thread, width.saturating_sub(MESSAGE_INDENT).max(1))
    }

    /// Width this layout was built for.
    pub(crate) fn width(&self) -> usize {
        self.width
    }

    /// Total rows for every message header and body.
    pub(crate) fn rows(&self) -> usize {
        self.rows
    }

    /// The row each message starts on.
    pub(crate) fn stops(&self) -> &[usize] {
        &self.stops
    }

    fn body(&self, message: usize) -> &Layout {
        self.bodies.body(message)
    }
}

/// A message: the cursor cell, the author's name in its kind's colour,
/// the age and an optional badge on one row, the body rendered as
/// Markdown and indented beneath it, every row on the kind's stripe
/// (ADR 0071); `selected` marks the cursor's message with the bar and
/// a bold name.
fn message_lines<'a>(
    theme: &Theme,
    message: &Message<'_>,
    body: &Layout,
    now: u64,
    width: usize,
    selected: bool,
) -> Vec<Line<'a>> {
    let row = row_style(theme, message.author);
    let mut header = vec![
        cursor_cell(theme, selected),
        Span::raw(" "),
        Span::styled(
            message.name.to_owned(),
            name_style(theme, message.author, selected),
        ),
        Span::styled(
            format!("  {}", format_age(message.created, now)),
            theme.info,
        ),
    ];
    if let Some(badge) = message.badge {
        // The badge in the author's colour: an agent's proposal reads
        // green as the agent's name does (ADR 0071).
        header.push(Span::styled(
            format!("  [{badge}]"),
            name_style(theme, message.author, false),
        ));
    }
    let mut out = vec![message_line(header, width, row)];
    let indent = " ".repeat(MESSAGE_INDENT - 1);
    for line in body.lines() {
        let mut spans = vec![cursor_cell(theme, selected), Span::raw(indent.clone())];
        spans.extend(
            line.spans()
                .iter()
                .map(|span| Span::styled(span.text().to_owned(), face_style(theme, span.style()))),
        );
        out.push(message_line(spans, width, row));
    }
    out
}

/// One row of a message, padded to the text width so `row`, the
/// stripe, reaches the right edge however short the row is.
pub(crate) fn message_line(mut spans: Vec<Span<'_>>, width: usize, row: Style) -> Line<'_> {
    let used = spans
        .iter()
        .map(|span| display_width(&span.content))
        .sum::<usize>();
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    Line::from(spans).style(row)
}

/// The rows of `thread` expanded in place (ADR 0049): the comment and
/// each reply as a message, author row then body, with no snippet and
/// no END row; `selected` is the message the cursor is on, and `user`
/// names the user (ADR 0058).
pub(crate) fn expanded_lines<'a>(
    theme: &Theme,
    thread: &Thread,
    layout: &ExpandedLayout,
    user: &str,
    now: u64,
    width: usize,
    selected: Option<usize>,
) -> Vec<Line<'a>> {
    let mut out = Vec::new();
    // Every author as `author_label` names them (ADR 0058, ADR 0061):
    // the configured name for the user, `name (type)` for an agent.
    let comment_name = author_label(thread.author(), user);
    let comment = Message {
        author: thread.author(),
        name: &comment_name,
        created: thread.created(),
        badge: None,
    };
    out.extend(message_lines(
        theme,
        &comment,
        layout.body(0),
        now,
        width,
        selected == Some(0),
    ));
    for (index, reply) in thread.replies().iter().enumerate() {
        let name = author_label(reply.author(), user);
        let message = Message {
            author: reply.author(),
            name: &name,
            created: reply.created(),
            badge: reply.proposes_resolution().then_some("proposes resolving"),
        };
        out.extend(message_lines(
            theme,
            &message,
            layout.body(index + 1),
            now,
            width,
            selected == Some(index + 1),
        ));
    }
    out
}

/// The body laid out as Markdown in the cells left of `width` after the
/// indent, a newline kept as a line break, fenced code coloured by
/// `highlighter`.
#[cfg(test)]
fn body_layout(body: &str, width: usize, highlighter: &Highlighter) -> Layout {
    Layout::render_message(
        body,
        width.saturating_sub(MESSAGE_INDENT).max(1),
        highlighter,
    )
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

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

    /// The same rows without the padding that fills each one out.
    fn trimmed(lines: &[Line<'_>]) -> Vec<String> {
        texts(lines)
            .into_iter()
            .map(|line| line.trim_end().to_owned())
            .collect()
    }

    fn render(body: &str, badge: Option<&str>, width: usize) -> anyhow::Result<Vec<Line<'static>>> {
        let author = Author::agent("Copilot");
        let message = Message {
            author: &author,
            name: "Copilot",
            created: 0,
            badge,
        };
        let body = body_layout(body, width, &Highlighter::plain());
        Ok(message_lines(&theme()?, &message, &body, 0, width, false))
    }

    #[test]
    fn plain_sentence_renders_as_itself_under_the_header() -> anyhow::Result<()> {
        let lines = render("please check this", None, 40)?;
        assert_eq!(
            trimmed(&lines),
            ["  Copilot  just now", "    please check this"]
        );
        Ok(())
    }

    #[test]
    fn a_newline_stays_a_line_break() -> anyhow::Result<()> {
        let lines = render("first\nsecond", None, 40)?;
        assert_eq!(trimmed(&lines)[1..], ["    first", "    second"]);
        Ok(())
    }

    #[test]
    fn markdown_blocks_render_and_wrap_to_the_indented_width() -> anyhow::Result<()> {
        let body = "Two **points**:\n\n- first\n- second `x`\n\n```rust\nfn a() {}\n```\n";
        let lines = render(body, Some("proposes resolving"), 30)?;
        let rows = trimmed(&lines);
        assert_eq!(rows[0], "  Copilot  just now  [proposes resolving]");
        assert!(rows.iter().any(|t| t == "    Two points:"), "{rows:?}");
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
        assert!(texts(&wrapped).iter().all(|t| display_width(t) == 30));
        Ok(())
    }

    #[test]
    fn every_row_fills_the_width_so_the_background_reaches_the_edge() -> anyhow::Result<()> {
        let lines = render("short", None, 24)?;
        assert!(texts(&lines).iter().all(|line| display_width(line) == 24));
        Ok(())
    }

    /// ADR 0071: the cursor's message keeps its author's stripe and
    /// gets the bar down its rows and a bold name; another author's
    /// message sits on its own stripe with no bar.
    #[test]
    fn a_selected_message_keeps_its_stripe_and_gets_the_bar() -> anyhow::Result<()> {
        let theme = theme()?;
        let message = Message {
            author: &Author::User,
            name: "User",
            created: 0,
            badge: None,
        };
        let body = body_layout("selected\nrows", 24, &Highlighter::plain());
        let lines = message_lines(&theme, &message, &body, 0, 24, true);
        assert!(
            lines
                .iter()
                .all(|line| line.style.bg == theme.thread_user.bg)
        );
        assert!(texts(&lines).iter().all(|line| display_width(line) == 24));
        assert!(
            texts(&lines).iter().all(|line| line.starts_with("▎")),
            "{:?}",
            texts(&lines)
        );
        let name = lines[0].spans.iter().find(|s| s.content == "User");
        assert!(
            name.is_some_and(|s| s.style.add_modifier.contains(Modifier::BOLD)
                && s.style.fg == theme.thread_user.fg),
            "bold in the user's colour"
        );
        let agent = Author::agent("coder");
        let other = Message {
            author: &agent,
            name: "coder",
            created: 0,
            badge: None,
        };
        let body = body_layout("theirs", 24, &Highlighter::plain());
        let lines = message_lines(&theme, &other, &body, 0, 24, false);
        assert!(
            lines
                .iter()
                .all(|line| line.style.bg == theme.thread_agent.bg)
        );
        assert!(texts(&lines).iter().all(|line| line.starts_with(' ')));
        Ok(())
    }
}
