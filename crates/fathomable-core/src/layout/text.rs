// @okf-doc: /decisions/0004-markdown-rendering.md
//! Byte-offset to source-line lookup and display-width helpers.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Maps byte offsets in a document to 1-based source line numbers.
///
/// The gutter shows source line numbers ([ADR 0010](../../docs/decisions/0010-viewer-ux.md))
/// and live reload re-anchors the cursor by source line, so every layout
/// carries one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    /// Byte offset at which each line starts; `starts[0] == 0`.
    starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    /// Index the line starts of `text`.
    #[must_use]
    pub fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self {
            starts,
            len: text.len(),
        }
    }

    /// Number of lines; a trailing newline does not start a new line.
    #[must_use]
    pub fn line_count(&self) -> usize {
        let count = self.starts.len();
        if count > 1 && self.starts[count - 1] == self.len {
            count - 1
        } else {
            count
        }
    }

    /// The 1-based line containing byte `offset`.
    ///
    /// Offsets past the end map to the last line.
    #[must_use]
    pub fn line_of(&self, offset: usize) -> usize {
        let idx = self.starts.partition_point(|&start| start <= offset);
        idx.clamp(1, self.line_count())
    }

    /// The byte range of 1-based `line`, excluding its newline.
    ///
    /// Returns `None` when `line` is out of range.
    #[must_use]
    pub fn range_of(&self, line: usize) -> Option<Range<usize>> {
        if line == 0 || line > self.line_count() {
            return None;
        }
        let start = self.starts[line - 1];
        let end = self.starts.get(line).map_or(self.len, |next| next - 1);
        Some(start..end.max(start))
    }

    /// The byte offset of display column `column` on 1-based `line`.
    ///
    /// Columns past the end of the line map to its end; `None` when `line`
    /// is out of range.
    #[must_use]
    pub fn offset_at(&self, text: &str, line: usize, column: usize) -> Option<usize> {
        let range = self.range_of(line)?;
        let slice = text.get(range.clone())?;
        let mut cells = 0;
        for (offset, grapheme) in graphemes(slice) {
            if cells >= column {
                return Some(range.start + offset);
            }
            cells += display_width(grapheme);
        }
        Some(range.end)
    }

    /// The 0-based display column of `offset` within its line.
    #[must_use]
    pub fn column_of(&self, text: &str, offset: usize) -> usize {
        let line = self.line_of(offset);
        let start = self.starts[line - 1];
        let end = offset.min(text.len()).max(start);
        text.get(start..end).map_or(0, display_width)
    }
}

/// Terminal cell width of `text`.
#[must_use]
pub fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Iterate `text` as `(byte_offset, grapheme)` pairs.
pub fn graphemes(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.grapheme_indices(true)
}
