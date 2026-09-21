// @okf-doc: /decisions/0004-markdown-rendering.md
//! Markdown layout: rendered lines with styled spans and source ranges.
//!
//! [`Layout::render`] lays a Markdown document out for a pane of a given
//! width; [`Layout::source`] lays the raw text out instead. Both produce
//! [`Line`]s of [`Span`]s, where every span that came from the source
//! remembers the byte range it was produced from, so a frontend can map a
//! cell back to the source for line numbers, selection, and annotations.
//!
//! Layout is pure data in, data out: it is deterministic for a given text and
//! width and touches no terminal.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::layout::{Face, Layout};
//!
//! let layout = Layout::render("# Title\n\nHello *world*.\n", 40);
//! let lines = layout.lines();
//! assert_eq!(lines[0].text(), "Title");
//! assert_eq!(lines[0].spans()[0].style().face, Face::Heading(1));
//! assert_eq!(lines[0].source_line(), Some(1));
//! ```

mod blocks;
mod text;
mod wrap;

use std::ops::Range;

use crate::diff::{Compare, DiffKind};
use crate::highlight::{Highlighter, Highlights};
use crate::theme::Color;

use blocks::{Align, Block, Inline, Item, Table};
#[doc(inline)]
pub use text::{LineIndex, display_width, graphemes};
use wrap::{Chunk, wrap, wrap_code_chunks, wrap_hard, wrap_hard_chunks};

/// What a single newline inside a paragraph means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Breaks {
    /// A soft break as in the Markdown spec, laid out as a space.
    #[default]
    Soft,
    /// A line break, as comments on a code host render them (ADR 0037).
    Hard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodeWrapping {
    Hard,
    AtWords,
}

/// What a span is, for theming.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Face {
    /// Body text.
    #[default]
    Text,
    /// Heading text of the given level, 1 through 6.
    Heading(u8),
    /// Inline code.
    Code,
    /// A line of a fenced or indented code block.
    CodeBlock,
    /// Link text (or image alt text) pointing at the URL.
    Link(String),
    /// Layout chrome: list bullets, table borders, quote bars, rules, task boxes.
    Marker,
    /// Quoted body text.
    Quote,
    /// A line added against the diff source (ADR 0006).
    DiffAdded,
    /// A line removed against the diff source.
    DiffRemoved,
    /// A unified-diff hunk header.
    DiffHeader,
}

/// Visual attributes of a span.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Style {
    /// What the span is.
    pub face: Face,
    /// Italic emphasis.
    pub emphasis: bool,
    /// Bold emphasis.
    pub strong: bool,
    /// Struck-through text.
    pub strikethrough: bool,
    /// A syntax-highlighting foreground that overrides the face's colour
    /// (ADR 0016).
    pub fg: Option<Color>,
}

impl Style {
    fn marker() -> Self {
        Self {
            face: Face::Marker,
            ..Self::default()
        }
    }
}

/// A run of text with one style on one rendered line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    text: String,
    style: Style,
    source: Option<Range<usize>>,
}

impl Span {
    /// The rendered text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The style to draw the text in.
    #[must_use]
    pub fn style(&self) -> &Style {
        &self.style
    }

    /// The source byte range this text came from; `None` for synthesised chrome.
    #[must_use]
    pub fn source(&self) -> Option<Range<usize>> {
        self.source.clone()
    }

    /// Terminal width of the text.
    #[must_use]
    pub fn width(&self) -> usize {
        display_width(&self.text)
    }

    /// The source byte offset of the grapheme at display column `col`.
    ///
    /// Columns beyond the text map to the end of the range. When the source
    /// and the text do not line up byte-for-byte (escapes, entities), every
    /// column maps to the start of the range.
    #[must_use]
    pub fn source_at(&self, col: usize) -> Option<usize> {
        let source = self.source.clone()?;
        if source.len() != self.text.len() {
            return Some(source.start);
        }
        let mut cells = 0;
        for (offset, grapheme) in text::graphemes(&self.text) {
            let width = display_width(grapheme);
            if cells + width > col {
                return Some(source.start + offset);
            }
            cells += width;
        }
        Some(source.end)
    }
}

/// One rendered line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    spans: Vec<Span>,
    source: Option<Range<usize>>,
    number: Option<usize>,
    /// Original-side line represented by a unified-diff row.
    old_diff: Option<usize>,
    /// Updated-side line represented by a unified-diff row.
    new_diff: Option<usize>,
    /// A blank row inserted before this source line by
    /// [`Layout::with_rows_before`] (ADR 0039).
    before: Option<usize>,
    /// A sourceless row inserted by [`Layout::with_rows_after`] (ADR
    /// 0049): the block it belongs to and its index within the block.
    stub: Option<(usize, usize)>,
}

/// Where a block of inserted rows hangs (ADR 0049).
///
/// A block sits under the last row of a source line, or under the blank
/// row standing before a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RowAnchor {
    /// The 1-based source line whose last rendered row the block follows.
    Line(usize),
    /// The detached row inserted before this 1-based source line.
    Detached(usize),
    /// Before the first row: a thread on the file as a whole (ADR 0063).
    Top,
}

impl Line {
    fn from_spans(spans: Vec<Span>) -> Self {
        let source = span_range(&spans);
        Self {
            spans,
            source,
            number: None,
            old_diff: None,
            new_diff: None,
            before: None,
            stub: None,
        }
    }

    fn blank() -> Self {
        Self::from_spans(Vec::new())
    }

    fn prefixed(mut self, prefix: &str) -> Self {
        if !prefix.is_empty() {
            self.spans.insert(
                0,
                Span {
                    text: prefix.to_owned(),
                    style: Style::marker(),
                    source: None,
                },
            );
        }
        self
    }

    /// The styled spans, left to right.
    #[must_use]
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// The whole source byte range this line was produced from.
    #[must_use]
    pub fn source(&self) -> Option<Range<usize>> {
        self.source.clone()
    }

    /// The 1-based source line this blank row was inserted before by
    /// [`Layout::with_rows_before`], one past the last line for a row
    /// appended at the end; `None` for a line of the document.
    #[must_use]
    pub fn stands_before(&self) -> Option<usize> {
        self.before
    }

    /// The block and index of a row [`Layout::with_rows_after`] inserted,
    /// `None` for a line of the document or a detached row.
    #[must_use]
    pub fn stub_slot(&self) -> Option<(usize, usize)> {
        self.stub
    }

    /// The 1-based source line to show in the gutter.
    ///
    /// `None` for wrapped continuations and synthesised lines, per ADR 0010.
    #[must_use]
    pub fn source_line(&self) -> Option<usize> {
        self.number
    }

    /// The 1-based original-side line represented by this diff row.
    ///
    /// Wrapped continuations retain the same line. Non-diff rows and hunk
    /// headers return `None`.
    #[must_use]
    pub fn diff_old_line(&self) -> Option<usize> {
        self.old_diff
    }

    /// The 1-based updated-side line represented by this diff row.
    ///
    /// Wrapped continuations retain the same line. Non-diff rows and hunk
    /// headers return `None`.
    #[must_use]
    pub fn diff_new_line(&self) -> Option<usize> {
        self.new_diff
    }

    /// The plain text of the line.
    #[must_use]
    pub fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    /// Terminal width of the line.
    #[must_use]
    pub fn width(&self) -> usize {
        self.spans.iter().map(Span::width).sum()
    }

    /// Display columns at which each grapheme cluster starts.
    ///
    /// Cursor motion steps through these so a wide character is never split.
    #[must_use]
    pub fn columns(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut col = 0;
        for span in &self.spans {
            for (_, grapheme) in text::graphemes(&span.text) {
                out.push(col);
                col += display_width(grapheme);
            }
        }
        out
    }

    /// The byte offset in [`Self::text`] of the grapheme under display
    /// column `col`, or `None` past the end of the line.
    #[must_use]
    pub fn byte_at(&self, col: usize) -> Option<usize> {
        let mut cells = 0;
        let mut bytes = 0;
        for span in &self.spans {
            for (offset, grapheme) in text::graphemes(&span.text) {
                let width = display_width(grapheme);
                if col < cells + width {
                    return Some(bytes + offset);
                }
                cells += width;
            }
            bytes += span.text.len();
        }
        None
    }

    /// The source byte offset under display column `col`, if any.
    ///
    /// Falls back to the nearest span with a source range so that clicking on
    /// chrome still lands somewhere sensible.
    #[must_use]
    pub fn source_at(&self, col: usize) -> Option<usize> {
        let mut cells = 0;
        for span in &self.spans {
            let width = span.width();
            if col < cells + width {
                // Inside this span; chrome without a source falls through to
                // the next sourced span so clicks on a bullet land on its text.
                if let Some(offset) = span.source_at(col.saturating_sub(cells)) {
                    return Some(offset);
                }
            } else if col < cells
                && let Some(range) = &span.source
            {
                return Some(range.start);
            }
            cells += width;
        }
        self.source.as_ref().map(|range| range.end)
    }
}

fn span_range(spans: &[Span]) -> Option<Range<usize>> {
    let mut range: Option<Range<usize>> = None;
    for source in spans.iter().filter_map(|span| span.source.clone()) {
        range = Some(match range {
            Some(existing) => existing.start.min(source.start)..existing.end.max(source.end),
            None => source,
        });
    }
    range
}

/// A document laid out for one pane width.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    lines: Vec<Line>,
    width: usize,
    index: LineIndex,
}

impl Layout {
    /// Lay `text` out as rendered Markdown wrapped to `width` cells, with
    /// code blocks left plain.
    #[must_use]
    pub fn render(text: &str, width: usize) -> Self {
        Self::render_with(text, width, &Highlighter::plain())
    }

    /// Lay `text` out as rendered Markdown wrapped to `width` cells,
    /// colouring fenced code blocks by their info string (ADR 0016).
    #[must_use]
    pub fn render_with(text: &str, width: usize, highlighter: &Highlighter) -> Self {
        Self::render_breaks(text, width, highlighter, Breaks::Soft, CodeWrapping::Hard)
    }

    /// Lay `text` out as a comment: rendered Markdown in which a single
    /// newline is a line break and fenced blocks prefer word boundaries.
    #[must_use]
    pub fn render_message(text: &str, width: usize, highlighter: &Highlighter) -> Self {
        Self::render_breaks(
            text,
            width,
            highlighter,
            Breaks::Hard,
            CodeWrapping::AtWords,
        )
    }

    fn render_breaks(
        text: &str,
        width: usize,
        highlighter: &Highlighter,
        breaks: Breaks,
        code_wrapping: CodeWrapping,
    ) -> Self {
        let index = LineIndex::new(text);
        let mut renderer = Renderer {
            text,
            width: width.max(1),
            lines: Vec::new(),
            highlighter,
            code_wrapping,
        };
        renderer.blocks(&blocks::parse(text, breaks), "", "");
        while renderer.lines.last().is_some_and(is_blank) {
            renderer.lines.pop();
        }
        Self::finish(renderer.lines, width, index)
    }

    /// Lay `text` out verbatim, wrapping long source lines to `width`.
    #[must_use]
    pub fn source(text: &str, width: usize) -> Self {
        Self::source_with(text, width, "", &Highlighter::plain())
    }

    /// Lay `text` out verbatim as [`Self::source`], coloured as the language
    /// `hint` names (a file extension or fence token, ADR 0016). An unknown
    /// hint leaves the text plain.
    #[must_use]
    pub fn source_with(text: &str, width: usize, hint: &str, highlighter: &Highlighter) -> Self {
        let runs = highlighter.highlight(text, hint);
        Self::source_with_highlights(text, width, runs.as_ref())
    }

    /// Lay `text` out as source using previously computed highlighting.
    ///
    /// `None` lays the text out without syntax colours.
    #[must_use]
    pub fn source_with_highlights(
        text: &str,
        width: usize,
        highlights: Option<&Highlights>,
    ) -> Self {
        let index = LineIndex::new(text);
        let runs = highlights.map(Highlights::lines);
        let mut lines = Vec::new();
        for line in 1..=index.line_count() {
            let Some(range) = index.range_of(line) else {
                continue;
            };
            let source = &text[range.clone()];
            let line_runs = runs.and_then(|runs| runs.get(line - 1));
            let chunks = coloured_chunks(source, range.start, &Style::default(), line_runs);
            lines.extend(wrap_hard_chunks(&chunks, width));
        }
        Self::finish(lines, width, index)
    }

    /// Lay out one sourceless marker notice.
    #[must_use]
    pub fn notice(text: &str, width: usize) -> Self {
        let chunk = Chunk::new(text, Style::marker(), None);
        let lines = wrap_hard(&chunk, width.max(1));
        Self::finish(lines, width, LineIndex::new(""))
    }

    /// Lay out a unified diff of `old` against `new`.
    ///
    /// Context and added lines carry updated-text source ranges. Removed
    /// lines and hunk headers have no updated-text source. Every diff row
    /// retains exact original and updated line identities, and wrapped
    /// continuations keep the same identities. When the texts are identical
    /// the layout is one sourceless notice. `compare` controls whitespace and
    /// context lines.
    #[must_use]
    pub fn diff(old: &str, new: &str, width: usize, compare: Compare) -> Self {
        let index = LineIndex::new(new);
        let diff = crate::diff::Diff::compare(old, new, compare.whitespace);
        let mut lines = Vec::new();
        if diff.is_empty() {
            let chunk = Chunk::new("no changes against the diff source", Style::marker(), None);
            lines.extend(wrap_hard(&chunk, width));
            return Self::finish(lines, width, index);
        }
        for entry in diff.unified(old, new, compare.context) {
            let old_line = entry.old_line();
            let new_line = entry.new_line();
            let (face, sign) = match entry.kind() {
                DiffKind::Header => (Face::DiffHeader, ""),
                DiffKind::Context => (Face::Text, " "),
                DiffKind::Added => (Face::DiffAdded, "+"),
                DiffKind::Removed => (Face::DiffRemoved, "-"),
            };
            let style = Style {
                face,
                ..Style::default()
            };
            let source = entry.new_line().and_then(|line| index.range_of(line));
            let chunk = Chunk::new(entry.text(), style.clone(), source);
            let content_width = width.saturating_sub(display_width(sign)).max(1);
            for (part, mut line) in wrap_hard(&chunk, content_width).into_iter().enumerate() {
                if !sign.is_empty() {
                    let prefix = if part == 0 { sign } else { " " };
                    line.spans.insert(
                        0,
                        Span {
                            text: prefix.to_owned(),
                            style: style.clone(),
                            source: None,
                        },
                    );
                }
                line.old_diff = old_line;
                line.new_diff = new_line;
                lines.push(line);
            }
        }
        Self::finish(lines, width, index)
    }

    fn finish(mut lines: Vec<Line>, width: usize, index: LineIndex) -> Self {
        let mut previous = None;
        for line in &mut lines {
            let number = line.source.as_ref().map(|range| index.line_of(range.start));
            line.number = number.filter(|&n| Some(n) != previous);
            if number.is_some() {
                previous = number;
            }
        }
        Self {
            lines,
            width,
            index,
        }
    }

    /// The layout with one blank row inserted before each source line in
    /// `lines` (ADR 0039): before the first rendered row whose source
    /// begins at or after the line, or at the end when there is none.
    /// Repeated lines share one row; [`Line::stands_before`] names it.
    #[must_use]
    pub fn with_rows_before(mut self, lines: &[usize]) -> Self {
        let mut anchors: Vec<usize> = lines.to_vec();
        anchors.sort_unstable();
        anchors.dedup();
        // Back to front, so earlier insertions do not shift later rows.
        for anchor in anchors.into_iter().rev() {
            let row = self
                .lines
                .iter()
                .position(|line| {
                    line.source
                        .as_ref()
                        .is_some_and(|range| self.index.line_of(range.start) >= anchor)
                })
                .unwrap_or(self.lines.len());
            let mut blank = Line::blank();
            blank.before = Some(anchor);
            self.lines.insert(row, blank);
        }
        self
    }

    /// Insert original-side removed rows at a target-side insertion boundary.
    ///
    /// These rows are synthetic with respect to this layout's source text,
    /// but retain their exact original-side line identity for navigation.
    #[must_use]
    pub fn with_old_deletion(
        mut self,
        old: &str,
        old_range: Range<usize>,
        insertion_line: usize,
    ) -> Self {
        let old_index = LineIndex::new(old);
        let at = self
            .lines
            .iter()
            .position(|line| {
                line.source
                    .as_ref()
                    .is_some_and(|range| self.index.line_of(range.start) >= insertion_line)
            })
            .unwrap_or(self.lines.len());
        let mut removed = Vec::new();
        for old_line in old_range {
            let Some(range) = old_index.range_of(old_line + 1) else {
                continue;
            };
            let style = Style {
                face: Face::DiffRemoved,
                ..Style::default()
            };
            let chunk = Chunk::new(&old[range], style.clone(), None);
            let content_width = self.width.saturating_sub(1).max(1);
            for (part, mut line) in wrap_hard(&chunk, content_width).into_iter().enumerate() {
                line.spans.insert(
                    0,
                    Span {
                        text: if part == 0 { "-" } else { " " }.to_owned(),
                        style: style.clone(),
                        source: None,
                    },
                );
                line.old_diff = Some(old_line + 1);
                removed.push(line);
            }
        }
        self.lines.splice(at..at, removed);
        self
    }

    /// Insert `count` sourceless rows after the row each anchor names
    /// (ADR 0049), block `i`'s rows carrying `(i, 0..count)`. Blocks
    /// are given in row order; two on one anchor come one after the
    /// other in that order. An anchor no row holds is skipped.
    #[must_use]
    pub fn with_rows_after(mut self, blocks: &[(RowAnchor, usize)]) -> Self {
        // Back to front, so earlier insertions do not shift later rows.
        for (block, &(anchor, count)) in blocks.iter().enumerate().rev() {
            let at = match anchor {
                RowAnchor::Top => 0,
                _ => match self.row_of_anchor(anchor) {
                    Some(row) => row + 1,
                    None => continue,
                },
            };
            for index in (0..count).rev() {
                let mut line = Line::blank();
                line.stub = Some((block, index));
                self.lines.insert(at, line);
            }
        }
        self
    }

    /// The last row holding `anchor`: the detached row before a line, or
    /// the last row whose source covers the line, else the last row that
    /// starts at or before it (a blank line has no row of its own).
    fn row_of_anchor(&self, anchor: RowAnchor) -> Option<usize> {
        match anchor {
            RowAnchor::Top => None,
            RowAnchor::Detached(before) => self
                .lines
                .iter()
                .rposition(|line| line.before == Some(before)),
            RowAnchor::Line(wanted) => {
                let lines_of = |line: &Line| {
                    let range = line.source.as_ref()?;
                    let first = self.index.line_of(range.start);
                    let last = self.index.line_of(range.end.max(range.start + 1) - 1);
                    Some((first, last))
                };
                self.lines
                    .iter()
                    .rposition(|line| {
                        lines_of(line)
                            .is_some_and(|(first, last)| first <= wanted && wanted <= last)
                    })
                    .or_else(|| {
                        self.lines.iter().rposition(|line| {
                            lines_of(line).is_some_and(|(first, _)| first <= wanted)
                        })
                    })
            }
        }
    }

    /// The rendered lines, top to bottom.
    #[must_use]
    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    /// The width the layout was produced for.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// The source line index of the laid-out text.
    #[must_use]
    pub fn index(&self) -> &LineIndex {
        &self.index
    }

    /// The rendered line containing source byte `offset`, or the nearest one.
    ///
    /// Used to re-anchor the cursor after a reload. Returns `None` only when
    /// no line carries a source range.
    #[must_use]
    pub fn line_at_offset(&self, offset: usize) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for (row, line) in self.lines.iter().enumerate() {
            let Some(range) = &line.source else {
                continue;
            };
            if range.contains(&offset) || (range.start == range.end && range.start == offset) {
                return Some(row);
            }
            // Ties prefer the line after the offset so a list marker maps to
            // its own item, not the previous one.
            let distance = if offset < range.start {
                (range.start - offset) * 2 - 1
            } else {
                (offset - range.end + 1) * 2
            };
            if best.is_none_or(|(_, d)| distance < d) {
                best = Some((row, distance));
            }
        }
        best.map(|(row, _)| row)
    }
}

fn is_blank(line: &Line) -> bool {
    line.spans.iter().all(|span| span.text.trim().is_empty()) && line.source.is_none()
}

struct Renderer<'a> {
    text: &'a str,
    width: usize,
    lines: Vec<Line>,
    highlighter: &'a Highlighter,
    code_wrapping: CodeWrapping,
}

impl Renderer<'_> {
    /// Emit `blocks`, prefixing the first line with `first` and the rest with `rest`.
    fn blocks(&mut self, blocks: &[Block], first: &str, rest: &str) {
        let mut prefix = first;
        for (i, block) in blocks.iter().enumerate() {
            // Tight list text runs straight into its nested list.
            if i > 0 && !matches!(blocks[i - 1], Block::Tight(_)) {
                self.push(Line::blank().prefixed(rest));
            }
            self.block(block, prefix, rest);
            prefix = rest;
        }
    }

    fn push(&mut self, line: Line) {
        self.lines.push(line);
    }

    fn avail(&self, prefix: &str) -> usize {
        self.width.saturating_sub(display_width(prefix)).max(1)
    }

    fn block(&mut self, block: &Block, first: &str, rest: &str) {
        match block {
            Block::Paragraph(inlines) | Block::Tight(inlines) => {
                self.paragraph(inlines, &Style::default(), first, rest);
            }
            Block::Heading(level, inlines) => {
                let style = Style {
                    face: Face::Heading(*level),
                    ..Style::default()
                };
                self.paragraph(inlines, &style, first, rest);
            }
            Block::Code { text, source, lang } => {
                self.code(text, source.clone(), lang, first, rest);
            }
            Block::Html { text, source } => {
                self.code(text, source.clone(), "", first, rest);
            }
            Block::List { start, items } => self.list(*start, items, first, rest),
            Block::Quote(blocks) => {
                let first = format!("{first}│ ");
                let rest = format!("{rest}│ ");
                self.blocks(blocks, &first, &rest);
            }
            Block::Table(table) => self.table(table, first, rest),
            Block::Rule(range) => {
                let rule = "─".repeat(self.avail(first));
                let mut line = Line::from_spans(vec![Span {
                    text: rule,
                    style: Style::marker(),
                    source: Some(range.clone()),
                }]);
                line.source = Some(range.clone());
                self.push(line.prefixed(first));
            }
            Block::Footnote { label, blocks } => {
                let marker = format!("[^{label}]: ");
                let first = format!("{first}{marker}");
                let rest = format!("{rest}{}", " ".repeat(display_width(&marker)));
                self.blocks(blocks, &first, &rest);
            }
        }
    }

    fn paragraph(&mut self, inlines: &[Inline], base: &Style, first: &str, rest: &str) {
        let chunks = chunks(inlines, base);
        let lines = wrap(&chunks, self.avail(first));
        self.emit(lines, first, rest);
    }

    fn emit(&mut self, lines: Vec<Line>, first: &str, rest: &str) {
        let mut prefix = first;
        for line in lines {
            self.push(line.prefixed(prefix));
            prefix = rest;
        }
    }

    /// Wrap code lines for this surface; a non-empty `lang` colours them.
    fn code(&mut self, text: &str, source: Range<usize>, lang: &str, first: &str, rest: &str) {
        let style = Style {
            face: Face::CodeBlock,
            ..Style::default()
        };
        let runs = if lang.is_empty() {
            None
        } else {
            self.highlighter.highlight(text, lang)
        };
        // Locate each rendered line inside the block's source so selection
        // maps to the exact bytes even when the fence is indented.
        let block = self.text.get(source.clone()).unwrap_or("");
        let mut cursor = 0;
        let mut prefix = first;
        for (index, line) in text.lines().enumerate() {
            let found = block.get(cursor..).and_then(|rest| rest.find(line));
            let range = found.map(|pos| {
                let start = source.start + cursor + pos;
                cursor = (cursor + pos + line.len() + 1).min(block.len());
                start..start + line.len()
            });
            let line_runs = runs
                .as_ref()
                .and_then(|highlights| highlights.lines().get(index));
            let chunks = match (&range, line_runs) {
                (Some(range), Some(line_runs)) if !line_runs.is_empty() => {
                    coloured_chunks(line, range.start, &style, Some(line_runs))
                }
                _ => vec![Chunk::new(
                    line,
                    style.clone(),
                    range.clone().or_else(|| Some(source.clone())),
                )],
            };
            let lines = match self.code_wrapping {
                CodeWrapping::Hard => wrap_hard_chunks(&chunks, self.avail(prefix)),
                CodeWrapping::AtWords => wrap_code_chunks(&chunks, self.avail(prefix)),
            };
            self.emit(lines, prefix, rest);
            prefix = rest;
        }
    }

    fn list(&mut self, start: Option<u64>, items: &[Item], first: &str, rest: &str) {
        let mut number = start;
        let loose = items
            .iter()
            .any(|item| matches!(item.blocks.first(), Some(Block::Paragraph(_))));
        for (i, item) in items.iter().enumerate() {
            if i > 0 && loose {
                self.push(Line::blank().prefixed(rest));
            }
            let marker = match number {
                Some(n) => {
                    number = Some(n + 1);
                    format!("{n}. ")
                }
                None => "• ".to_owned(),
            };
            let task = item
                .task
                .map(|done| format!("{} ", blocks::task_marker(done)));
            let head = format!("{marker}{}", task.as_deref().unwrap_or(""));
            let item_first = format!("{}{head}", if i == 0 { first } else { rest });
            let item_rest = format!("{rest}{}", " ".repeat(display_width(&head)));
            if item.blocks.is_empty() {
                self.push(Line::blank().prefixed(&item_first));
            } else {
                self.blocks(&item.blocks, &item_first, &item_rest);
            }
        }
    }

    fn table(&mut self, table: &Table, first: &str, rest: &str) {
        let columns = table
            .head
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if columns == 0 {
            return;
        }
        let rows: Vec<&Vec<Vec<Inline>>> = std::iter::once(&table.head)
            .chain(table.rows.iter())
            .collect();
        let empty = Vec::new();
        let cell = |row: &'_ Vec<Vec<Inline>>, col: usize| -> Vec<Inline> {
            row.get(col).unwrap_or(&empty).clone()
        };
        // Natural width of each column before cell wrapping.
        let mut widths: Vec<usize> = (0..columns)
            .map(|col| {
                rows.iter()
                    .map(|row| chunks_width(&chunks(&cell(row, col), &Style::default())))
                    .max()
                    .unwrap_or(0)
                    .max(1)
            })
            .collect();
        // Borders: one separator per column plus one, and one cell of padding
        // each side of every column.
        let chrome = columns * 3 + 1;
        let avail = self.avail(first).saturating_sub(chrome).max(columns);
        shrink(&mut widths, avail);
        // A table whose minimum columns still exceed the pane is hard-wrapped
        // row by row so every rendered line remains reachable.
        let overflows = widths.iter().sum::<usize>() + chrome > self.avail(first);

        let border = |left, fill, mid, right| table_border(&widths, left, fill, mid, right);
        let mut out = vec![border("┏", "━", "┯", "┓")];
        for (r, row) in rows.iter().enumerate() {
            let cells: Vec<Vec<Line>> = (0..columns)
                .map(|col| {
                    let style = if r == 0 {
                        Style {
                            strong: true,
                            ..Style::default()
                        }
                    } else {
                        Style::default()
                    };
                    wrap(&chunks(&cell(row, col), &style), widths[col])
                })
                .collect();
            let height = cells.iter().map(Vec::len).max().unwrap_or(1).max(1);
            for line_no in 0..height {
                let mut spans = vec![Span {
                    text: "┃ ".to_owned(),
                    style: Style::marker(),
                    source: None,
                }];
                for (col, lines) in cells.iter().enumerate() {
                    let line = lines.get(line_no);
                    let used = line.map_or(0, Line::width);
                    let pad = widths[col].saturating_sub(used);
                    let (left, right) = match table.align.get(col) {
                        Some(Align::Right) => (pad, 0),
                        Some(Align::Center) => (pad / 2, pad - pad / 2),
                        _ => (0, pad),
                    };
                    let push_pad = |n: usize, spans: &mut Vec<Span>| {
                        if n > 0 {
                            spans.push(Span {
                                text: " ".repeat(n),
                                style: Style::default(),
                                source: None,
                            });
                        }
                    };
                    push_pad(left, &mut spans);
                    if let Some(line) = line {
                        spans.extend(line.spans.iter().cloned());
                    }
                    push_pad(right, &mut spans);
                    spans.push(Span {
                        text: " │ ".to_owned(),
                        style: Style::marker(),
                        source: None,
                    });
                }
                if let Some(last) = spans.last_mut() {
                    " ┃".clone_into(&mut last.text);
                }
                out.push(Line::from_spans(spans));
            }
            if r == 0 {
                out.push(border("┣", "━", "┿", "┫"));
            } else if r + 1 < rows.len() {
                out.push(border("┠", "─", "┼", "┨"));
            }
        }
        out.push(border("┗", "━", "┷", "┛"));
        if overflows {
            out = out
                .into_iter()
                .flat_map(|line| hard_wrap_line(line, self.avail(first)))
                .collect();
        }
        self.emit(out, first, rest);
    }
}

fn hard_wrap_line(line: Line, width: usize) -> Vec<Line> {
    if line.width() <= width {
        return vec![line];
    }
    let chunks: Vec<Chunk> = line
        .spans
        .into_iter()
        .map(|span| Chunk::new(span.text, span.style, span.source))
        .collect();
    wrap_hard_chunks(&chunks, width)
}

/// A horizontal table rule: heavy lines frame the table and underline the
/// header; light lines separate body rows and columns.
fn table_border(widths: &[usize], left: &str, fill: &str, mid: &str, right: &str) -> Line {
    let body: Vec<String> = widths.iter().map(|w| fill.repeat(w + 2)).collect();
    Line::from_spans(vec![Span {
        text: format!("{left}{}{right}", body.join(mid)),
        style: Style::marker(),
        source: None,
    }])
}

/// Shrink the widest columns until they fit `avail`, never below three cells.
/// Split one source line into chunks, one per highlighter run plus plain
/// gaps, each carrying its source range offset by `base`.
fn coloured_chunks(
    line: &str,
    base: usize,
    style: &Style,
    runs: Option<&Vec<crate::highlight::Run>>,
) -> Vec<Chunk> {
    let mut pieces: Vec<(Range<usize>, Option<Color>)> = Vec::new();
    let mut at = 0;
    for run in runs.into_iter().flatten() {
        let end = run.range.end.min(line.len());
        if run.range.start > at {
            pieces.push((at..run.range.start.min(end), None));
        }
        pieces.push((run.range.start.max(at)..end, Some(run.fg)));
        at = at.max(end);
    }
    if at < line.len() || pieces.is_empty() {
        pieces.push((at..line.len(), None));
    }
    let chunks: Vec<Chunk> = pieces
        .into_iter()
        .filter(|(range, _)| {
            !range.is_empty()
                && line.is_char_boundary(range.start)
                && line.is_char_boundary(range.end)
        })
        .map(|(range, fg)| {
            let style = Style {
                fg,
                ..style.clone()
            };
            Chunk::new(
                &line[range.clone()],
                style,
                Some(base + range.start..base + range.end),
            )
        })
        .collect();
    if chunks.is_empty() {
        // An empty line keeps a zero-length range so the gutter numbers it.
        return vec![Chunk::new("", style.clone(), Some(base..base))];
    }
    chunks
}

fn shrink(widths: &mut [usize], avail: usize) {
    // A column keeps at least one glyph and one space of padding on each
    // side; narrower columns render as noise, so they win over fitting.
    const MIN: usize = 3;
    while widths.iter().sum::<usize>() > avail {
        let Some((idx, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, w)| **w > MIN)
            .max_by_key(|(_, w)| **w)
        else {
            break;
        };
        widths[idx] -= 1;
    }
}

fn chunks(inlines: &[Inline], base: &Style) -> Vec<Chunk> {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text {
                text,
                style,
                source,
            } => {
                let mut style = style.clone();
                if style.face == Face::Text {
                    style.face = base.face.clone();
                }
                style.strong |= base.strong;
                style.emphasis |= base.emphasis;
                Chunk::new(text.clone(), style, Some(source.clone()))
            }
            Inline::HardBreak => Chunk {
                hard_break: true,
                ..Chunk::new("", Style::default(), None)
            },
        })
        .collect()
}

fn chunks_width(chunks: &[Chunk]) -> usize {
    chunks
        .iter()
        .map(|chunk| display_width(chunk.text.trim_end()))
        .sum()
}
