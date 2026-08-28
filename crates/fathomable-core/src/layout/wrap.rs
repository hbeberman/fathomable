// @okf-doc: /decisions/0004-markdown-rendering.md
//! Word wrapping of styled, source-mapped text runs.

use std::ops::Range;

use super::text::{display_width, graphemes};
use super::{Line, Span, Style};

/// A run of text with one style and, when known, its source range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Chunk {
    pub text: String,
    pub style: Style,
    pub source: Option<Range<usize>>,
    /// Forces a line break after this chunk.
    pub hard_break: bool,
}

impl Chunk {
    pub(super) fn new(text: impl Into<String>, style: Style, source: Option<Range<usize>>) -> Self {
        Self {
            text: text.into(),
            style,
            source,
            hard_break: false,
        }
    }

    /// The sub-range of `source` covering bytes `bytes` of `text`.
    ///
    /// Source and rendered text only line up byte-for-byte when no escape or
    /// entity was decoded; otherwise the whole range is kept.
    fn sub_source(&self, bytes: Range<usize>) -> Option<Range<usize>> {
        let source = self.source.clone()?;
        if source.len() == self.text.len() {
            Some(source.start + bytes.start..source.start + bytes.end)
        } else {
            Some(source)
        }
    }
}

/// One wrap unit: a word with its trailing whitespace.
struct Word<'a> {
    chunk: &'a Chunk,
    bytes: Range<usize>,
    /// Width of the word without trailing whitespace.
    width: usize,
}

fn words(chunk: &Chunk) -> Vec<Word<'_>> {
    let mut out = Vec::new();
    let text = &chunk.text;
    let mut start = 0;
    let mut in_space = text.starts_with(char::is_whitespace);
    // Split at each transition from whitespace to non-whitespace.
    for (idx, ch) in text.char_indices() {
        let space = ch.is_whitespace();
        if in_space && !space && idx > start {
            out.push(word(chunk, start..idx));
            start = idx;
        }
        in_space = space;
    }
    if start < text.len() {
        out.push(word(chunk, start..text.len()));
    }
    out
}

fn word(chunk: &Chunk, bytes: Range<usize>) -> Word<'_> {
    let text = &chunk.text[bytes.clone()];
    let trimmed = text.trim_end();
    Word {
        chunk,
        width: display_width(trimmed),
        bytes,
    }
}

/// Accumulates spans for the line being built.
#[derive(Default)]
struct Builder {
    spans: Vec<Span>,
    width: usize,
}

impl Builder {
    fn push(&mut self, chunk: &Chunk, bytes: Range<usize>) {
        let text = &chunk.text[bytes.clone()];
        if text.is_empty() {
            return;
        }
        self.width += display_width(text);
        let source = chunk.sub_source(bytes);
        if let Some(last) = self.spans.last_mut()
            && last.style == chunk.style
            && joins(last.source.as_ref(), source.as_ref())
        {
            last.text.push_str(text);
            if let (Some(a), Some(b)) = (last.source.as_mut(), source) {
                a.end = b.end;
            }
            return;
        }
        self.spans.push(Span {
            text: text.to_owned(),
            style: chunk.style.clone(),
            source,
        });
    }

    fn finish(&mut self) -> Line {
        // Drop trailing whitespace so it neither counts toward width nor
        // shows as a selectable cell.
        if let Some(last) = self.spans.last_mut() {
            let trimmed = last.text.trim_end().len();
            if trimmed == 0 {
                self.spans.pop();
            } else {
                if let Some(source) = last.source.as_mut()
                    && source.len() == last.text.len()
                {
                    source.end = source.start + trimmed;
                }
                last.text.truncate(trimmed);
            }
        }
        let spans = std::mem::take(&mut self.spans);
        self.width = 0;
        Line::from_spans(spans)
    }
}

fn joins(a: Option<&Range<usize>>, b: Option<&Range<usize>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.end == b.start,
        (None, None) => true,
        _ => false,
    }
}

/// Wrap `chunks` to `width` cells, breaking at whitespace.
///
/// Words wider than the line are broken between grapheme clusters. A zero
/// width is treated as one cell so layout always terminates.
pub(super) fn wrap(chunks: &[Chunk], width: usize) -> Vec<Line> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = Builder::default();
    for chunk in chunks {
        for word in words(chunk) {
            if line.width > 0 && line.width + word.width > width {
                lines.push(line.finish());
            }
            if word.width <= width {
                line.push(word.chunk, word.bytes.clone());
            } else {
                break_word(&mut lines, &mut line, &word, width);
            }
        }
        if chunk.hard_break {
            lines.push(line.finish());
        }
    }
    if line.width > 0 || lines.is_empty() {
        lines.push(line.finish());
    }
    lines
}

fn break_word(lines: &mut Vec<Line>, line: &mut Builder, word: &Word<'_>, width: usize) {
    let text = &word.chunk.text[word.bytes.clone()];
    let mut start = 0;
    let mut used = line.width;
    for (offset, grapheme) in graphemes(text) {
        let cell = display_width(grapheme);
        if used + cell > width && used > 0 {
            line.push(
                word.chunk,
                word.bytes.start + start..word.bytes.start + offset,
            );
            lines.push(line.finish());
            start = offset;
            used = 0;
        }
        used += cell;
    }
    line.push(word.chunk, word.bytes.start + start..word.bytes.end);
}

/// Hard-wrap `chunk` between grapheme clusters, never at words.
pub(super) fn wrap_hard(chunk: &Chunk, width: usize) -> Vec<Line> {
    wrap_hard_chunks(std::slice::from_ref(chunk), width)
}

/// Join `chunks`, which together form one source line, into one line
/// that is never wrapped (ADR 0029).
pub(super) fn unwrapped(chunks: &[Chunk]) -> Line {
    wrap_hard_chunks(chunks, usize::MAX)
        .pop()
        .unwrap_or_else(|| Line::from_spans(Vec::new()))
}

/// Hard-wrap `chunks`, which together form one source line, between
/// grapheme clusters; a style change never forces a break.
pub(super) fn wrap_hard_chunks(chunks: &[Chunk], width: usize) -> Vec<Line> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = Builder::default();
    for chunk in chunks {
        let mut start = 0;
        for (offset, grapheme) in graphemes(&chunk.text) {
            let cell = display_width(grapheme);
            if line.width + cell > width && line.width > 0 {
                line.push(chunk, start..offset);
                lines.push(Line::from_spans(std::mem::take(&mut line.spans)));
                line.width = 0;
                start = offset;
            }
            line.width += cell;
        }
        let used = line.width;
        line.width = 0;
        line.push(chunk, start..chunk.text.len());
        line.width = used;
    }
    lines.push(Line::from_spans(std::mem::take(&mut line.spans)));
    // An empty source line still has a (zero-length) range for the gutter.
    if lines.len() == 1
        && lines[0].source.is_none()
        && let Some(first) = chunks.first()
    {
        lines[0].source.clone_from(&first.source);
    }
    lines
}
