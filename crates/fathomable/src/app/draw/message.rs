// @okf-doc: /decisions/0037-markdown-in-threads.md
//! Thread messages rendered as Markdown (ADR 0037), in the rows of an
//! expanded thread (ADR 0049).
//!
//! A comment or reply body goes through the same renderer as a Markdown
//! file, wrapped to the text width less the message indent, with fenced
//! code coloured by its language. File and Threads surfaces share retained
//! body layouts; inline row counts and drawing use those same layouts, so
//! navigation and drawing do not parse or highlight a body again.
//! Stored origin source uses the File view's anchor language: `╭│╰` for
//! a rendered range and the lifecycle circle for a one-row target.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(test)]
use std::cell::Cell;

use fathomable_core::annotations::{
    Author, LineRange, MAX_ORIGIN_EVIDENCE_BYTES, Thread, ThreadId,
};
use fathomable_core::highlight::{Highlighter, Highlights, language_hint};
use fathomable_core::layout::{Layout, Line as LayoutLine, display_width};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::app::draw::author::{THREAD_GUTTER, cursor_cell, name_style, row_style};
use crate::app::draw::{Theme, face_style, format_age};
use crate::app::threads::author_label;

/// Cells a message body sits in from the block's left edge: the
/// thread's own gutter (ADR 0071) and two more.
pub(crate) const MESSAGE_INDENT: usize = THREAD_GUTTER + 2;
const CACHED_WIDTHS_PER_THREAD: usize = 2;
/// Maximum logical source rows prepared for one immutable origin block.
pub(crate) const MAX_ORIGIN_CONTEXT_ROWS: usize = 256;
/// Cells between the main Threads edge and immutable source text.
pub(crate) const ORIGIN_CONTEXT_INDENT: usize = 4;
const SELECTED_SOURCE_OMITTED: &str = "… selected source omitted …";
pub(crate) const CAPTURED_CONTEXT_TRUNCATED: &str = "… captured context truncated …";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OriginLineKind {
    Context,
    Selected,
    Omitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OriginAnchor {
    Point,
    Start,
    Middle,
    End,
}

impl OriginAnchor {
    fn glyph(self, point: &'static str) -> &'static str {
        match self {
            Self::Point => point,
            Self::Start => "╭",
            Self::Middle => "│",
            Self::End => "╰",
        }
    }
}

#[derive(Debug, Clone)]
struct PreparedOrigin {
    path: PathBuf,
    range: LineRange,
    text: String,
    lines: Vec<OriginLineKind>,
    highlights: Option<Highlights>,
    truncated: bool,
}

impl PreparedOrigin {
    #[expect(
        clippy::too_many_lines,
        reason = "One bounded pass keeps logical selection, byte limits, and syntax input aligned."
    )]
    fn new(thread: &Thread, highlighter: &Highlighter) -> Option<Self> {
        let origin = thread.origin();
        let range = origin.range()?;
        let (before, selected, after, capture_truncated, capture_omitted) =
            origin.context().map_or_else(
                || {
                    let selected = if origin.snippet().is_empty() {
                        vec![""]
                    } else {
                        origin.snippet().split('\n').collect()
                    };
                    (Vec::new(), selected, Vec::new(), false, false)
                },
                |context| {
                    (
                        context
                            .before_lines()
                            .iter()
                            .rev()
                            .take(3)
                            .rev()
                            .map(String::as_str)
                            .collect(),
                        context
                            .selected_lines()
                            .iter()
                            .map(String::as_str)
                            .collect(),
                        context
                            .after_lines()
                            .iter()
                            .take(3)
                            .map(String::as_str)
                            .collect(),
                        context.is_truncated(),
                        context.omitted_selected_lines() > 0,
                    )
                },
            );
        let side_rows = before.len() + after.len();
        let selected_limit = MAX_ORIGIN_CONTEXT_ROWS.saturating_sub(side_rows);
        let omit_selected = capture_omitted || selected.len() > selected_limit;
        let mut logical = Vec::with_capacity(MAX_ORIGIN_CONTEXT_ROWS);
        logical.extend(
            before
                .into_iter()
                .map(|line| (line, OriginLineKind::Context)),
        );
        if omit_selected {
            let kept = selected.len().min(selected_limit.saturating_sub(1));
            let head = kept.div_ceil(2);
            let tail = kept - head;
            logical.extend(
                selected[..head]
                    .iter()
                    .copied()
                    .map(|line| (line, OriginLineKind::Selected)),
            );
            logical.push((SELECTED_SOURCE_OMITTED, OriginLineKind::Omitted));
            logical.extend(
                selected[selected.len().saturating_sub(tail)..]
                    .iter()
                    .copied()
                    .map(|line| (line, OriginLineKind::Selected)),
            );
        } else {
            logical.extend(
                selected
                    .iter()
                    .copied()
                    .map(|line| (line, OriginLineKind::Selected)),
            );
        }
        logical.extend(
            after
                .into_iter()
                .map(|line| (line, OriginLineKind::Context)),
        );

        let full_bytes = logical
            .iter()
            .map(|(line, _)| line.len())
            .fold(logical.len().saturating_sub(1), usize::saturating_add);
        let mut truncated = origin.evidence_truncated() || capture_truncated || omit_selected;
        let mut text = String::with_capacity(full_bytes.min(MAX_ORIGIN_EVIDENCE_BYTES));
        let mut lines = Vec::with_capacity(logical.len());
        let mut remaining = MAX_ORIGIN_EVIDENCE_BYTES;
        let total = logical.len();
        for (index, (line, kind)) in logical.into_iter().enumerate() {
            if index > 0 {
                text.push('\n');
                remaining = remaining.saturating_sub(1);
            }
            let rows_left = total - index;
            let separators_left = rows_left.saturating_sub(1);
            let content_budget = remaining.saturating_sub(separators_left);
            let fair_budget = if full_bytes > MAX_ORIGIN_EVIDENCE_BYTES {
                content_budget / rows_left.max(1)
            } else {
                content_budget
            };
            let end = utf8_prefix(line, line.len().min(fair_budget));
            text.push_str(&line[..end]);
            remaining = remaining.saturating_sub(end);
            truncated |= end < line.len();
            lines.push(kind);
        }
        let highlights = highlighter.highlight(&text, &language_hint(origin.path()));
        Some(Self {
            path: origin.path().to_path_buf(),
            range,
            text,
            lines,
            highlights,
            truncated,
        })
    }
}

fn utf8_prefix(text: &str, mut end: usize) -> usize {
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// One wrapped immutable-origin row and its selected-range treatment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OriginContextRow {
    pub(crate) line: LayoutLine,
    pub(crate) selected: bool,
    pub(crate) omitted: bool,
    anchor: Option<OriginAnchor>,
}

impl OriginContextRow {
    pub(crate) fn anchor_glyph(&self, point: &'static str) -> Option<&'static str> {
        self.anchor.map(|anchor| anchor.glyph(point))
    }
}

/// Width-dependent rows for one bounded, highlighted immutable origin.
#[derive(Debug, Clone)]
pub(crate) struct OriginContextLayout {
    width: usize,
    rows: Vec<OriginContextRow>,
    truncated: bool,
}

impl OriginContextLayout {
    fn new(prepared: &PreparedOrigin, width: usize) -> Self {
        let layout = Layout::source_with_highlights(
            &prepared.text,
            width.max(1),
            prepared.highlights.as_ref(),
        );
        let index = fathomable_core::layout::LineIndex::new(&prepared.text);
        let mut rows: Vec<_> = layout
            .lines()
            .iter()
            .map(|line| {
                let kind = line
                    .source_line()
                    .or_else(|| line.source().map(|source| index.line_of(source.start)))
                    .and_then(|line| prepared.lines.get(line.saturating_sub(1)))
                    .copied()
                    .unwrap_or(OriginLineKind::Context);
                OriginContextRow {
                    line: line.clone(),
                    selected: kind == OriginLineKind::Selected,
                    omitted: kind == OriginLineKind::Omitted,
                    anchor: None,
                }
            })
            .collect();
        if prepared.lines.len() > index.line_count() {
            let blank = Layout::source("", width.max(1)).lines()[0].clone();
            rows.extend(
                prepared.lines[index.line_count()..]
                    .iter()
                    .map(|kind| OriginContextRow {
                        line: blank.clone(),
                        selected: *kind == OriginLineKind::Selected,
                        omitted: *kind == OriginLineKind::Omitted,
                        anchor: None,
                    }),
            );
        }
        for index in 0..rows.len() {
            let anchored = rows[index].selected || rows[index].omitted;
            if !anchored {
                continue;
            }
            let above = index > 0 && (rows[index - 1].selected || rows[index - 1].omitted);
            let below = rows
                .get(index + 1)
                .is_some_and(|row| row.selected || row.omitted);
            rows[index].anchor = Some(match (above, below) {
                (false, false) => OriginAnchor::Point,
                (false, true) => OriginAnchor::Start,
                (true, true) => OriginAnchor::Middle,
                (true, false) => OriginAnchor::End,
            });
        }
        Self {
            width,
            rows,
            truncated: prepared.truncated,
        }
    }

    pub(crate) fn rows(&self) -> &[OriginContextRow] {
        &self.rows
    }

    pub(crate) fn is_truncated(&self) -> bool {
        self.truncated
    }
}

#[derive(Debug)]
struct CachedOrigin {
    path: PathBuf,
    range: LineRange,
    content_hash: Option<String>,
    context_shape: Option<(usize, usize, usize, bool)>,
    prepared: Arc<PreparedOrigin>,
    layouts: VecDeque<Arc<OriginContextLayout>>,
}

/// Retains immutable source highlighting separately from wrapped variants.
#[derive(Debug, Default)]
pub(crate) struct OriginContextCache {
    entries: RefCell<HashMap<ThreadId, CachedOrigin>>,
    #[cfg(test)]
    preparations: Cell<usize>,
}

impl OriginContextCache {
    pub(crate) fn layout(
        &self,
        thread: &Thread,
        width: usize,
        highlighter: &Highlighter,
    ) -> Option<Arc<OriginContextLayout>> {
        let origin = thread.origin();
        let range = origin.range()?;
        let content_hash = origin
            .content()
            .map(fathomable_core::annotations::ContentIdentity::hash);
        let context_shape = origin.context().map(|context| {
            (
                context.before_lines().len(),
                context.selected_lines().len(),
                context.after_lines().len(),
                context.is_truncated(),
            )
        });
        let mut entries = self.entries.borrow_mut();
        let stale = entries.get(thread.id()).is_some_and(|cached| {
            cached.path != origin.path()
                || cached.range != range
                || cached.content_hash.as_deref() != content_hash
                || cached.context_shape != context_shape
        });
        if stale {
            entries.remove(thread.id());
        }
        let cached = match entries.entry(thread.id().clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let prepared = Arc::new(PreparedOrigin::new(thread, highlighter)?);
                #[cfg(test)]
                self.preparations.set(self.preparations.get() + 1);
                entry.insert(CachedOrigin {
                    path: prepared.path.clone(),
                    range: prepared.range,
                    content_hash: content_hash.map(str::to_owned),
                    context_shape,
                    prepared,
                    layouts: VecDeque::new(),
                })
            }
        };
        let code_width = width.saturating_sub(ORIGIN_CONTEXT_INDENT).max(1);
        if let Some(at) = cached
            .layouts
            .iter()
            .position(|layout| layout.width == code_width)
            && let Some(layout) = cached.layouts.remove(at)
        {
            cached.layouts.push_back(Arc::clone(&layout));
            return Some(layout);
        }
        let layout = Arc::new(OriginContextLayout::new(&cached.prepared, code_width));
        if cached.layouts.len() >= CACHED_WIDTHS_PER_THREAD {
            cached.layouts.pop_front();
        }
        cached.layouts.push_back(Arc::clone(&layout));
        Some(layout)
    }

    #[cfg(test)]
    pub(crate) fn preparations(&self) -> usize {
        self.preparations.get()
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
mod tests;
