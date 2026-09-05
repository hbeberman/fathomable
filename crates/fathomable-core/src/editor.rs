// @okf-doc: /decisions/0018-comment-editor.md
//! A small text buffer with a cursor for the comment box (ADR 0018).
//!
//! [`Buffer`] holds the draft of a comment as plain text plus a cursor. It
//! knows nothing about terminals: keys become [`Edit`]s and pastes become
//! [`Buffer::insert`], and the box's rows, the terminal cursor, and the
//! target of a mouse click all come from the same wrapping
//! ([`Buffer::rows`], [`Buffer::cursor_cell`], [`Buffer::place_cursor`]),
//! so the whole behaviour is testable as data.
//!
//! Left and Right step by grapheme; Up and Down keep a wanted column
//! across shorter lines; a word is a run of non-whitespace.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::editor::{Buffer, Cursor, Edit, Motion};
//!
//! let mut buffer = Buffer::new();
//! buffer.insert("tighten this");
//! buffer.apply(Edit::DeleteWordBack);
//! buffer.insert("that");
//! buffer.apply(Edit::Move(Motion::LineStart));
//! buffer.insert("please ");
//! assert_eq!(buffer.text(), "please tighten that");
//! assert_eq!(buffer.cursor(), Cursor { line: 0, column: 7 });
//! ```

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Where the cursor sits: a line index and a column counted in graphemes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Cursor {
    /// The 0-based line.
    pub line: usize,
    /// The 0-based column, counted in graphemes.
    pub column: usize,
}

/// A cursor motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Motion {
    /// One grapheme left, wrapping to the end of the previous line.
    Left,
    /// One grapheme right, wrapping to the start of the next line.
    Right,
    /// One line up, keeping the column where it can.
    Up,
    /// One line down, keeping the column where it can.
    Down,
    /// To the start of the line.
    LineStart,
    /// To the end of the line.
    LineEnd,
    /// To the start of the previous word.
    WordBack,
    /// To the start of the next word.
    WordForward,
}

/// An operation on a [`Buffer`] other than inserting text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Edit {
    /// Move the cursor.
    Move(Motion),
    /// Insert a line break at the cursor.
    Newline,
    /// Delete the grapheme before the cursor.
    DeleteBack,
    /// Delete the grapheme under the cursor.
    DeleteForward,
    /// Delete back to the start of the previous word.
    DeleteWordBack,
    /// Delete back to the start of the line.
    DeleteToLineStart,
    /// Delete forward to the end of the line.
    DeleteToLineEnd,
}

/// One wrapped row of a buffer laid out at a width.
///
/// A row belongs to one line and shows a byte range of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    line: usize,
    start: usize,
    end: usize,
}

impl Row {
    /// The index of the line this row shows part of.
    #[must_use]
    pub fn line(&self) -> usize {
        self.line
    }
}

/// A cell in the wrapped layout: a row index and a display column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Cell {
    /// The 0-based wrapped row.
    pub row: usize,
    /// The 0-based display column.
    pub column: usize,
}

/// Text with a cursor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Buffer {
    text: String,
    /// Byte offset of the cursor, always on a char boundary.
    at: usize,
    /// The column Up/Down aim for, kept across shorter lines.
    want: Option<usize>,
}

impl Buffer {
    /// An empty buffer with the cursor at the start.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A buffer holding `text` with the cursor at its end.
    #[must_use]
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        let at = text.len();
        Self {
            text,
            at,
            want: None,
        }
    }

    /// The whole text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the buffer holds no text.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// The lines of the text; an empty buffer has one empty line.
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.text.split('\n')
    }

    /// The cursor as a line index and a grapheme column.
    #[must_use]
    pub fn cursor(&self) -> Cursor {
        let before = &self.text[..self.at];
        let line = before.matches('\n').count();
        let column = before
            .rsplit_once('\n')
            .map_or(before, |(_, tail)| tail)
            .graphemes(true)
            .count();
        Cursor { line, column }
    }

    /// Insert `text` at the cursor and move past it; newlines are kept.
    pub fn insert(&mut self, text: &str) {
        self.text.insert_str(self.at, text);
        self.at += text.len();
        self.want = None;
    }

    /// Apply an edit.
    pub fn apply(&mut self, edit: Edit) {
        match edit {
            Edit::Move(motion) => self.step(motion),
            Edit::Newline => self.insert("\n"),
            Edit::DeleteBack => {
                let start = self.prev_grapheme();
                self.remove(start..self.at);
            }
            Edit::DeleteForward => {
                let end = self.next_grapheme();
                self.remove(self.at..end);
            }
            Edit::DeleteWordBack => {
                let start = self.word_back();
                self.remove(start..self.at);
            }
            Edit::DeleteToLineStart => {
                let start = self.line_start();
                self.remove(start..self.at);
            }
            Edit::DeleteToLineEnd => {
                let end = self.line_end();
                self.remove(self.at..end);
            }
        }
    }

    /// Move the cursor to `line` and grapheme `column`, clamped to the text.
    pub fn set_cursor(&mut self, cursor: Cursor) {
        let mut start = 0;
        let mut at = self.text.len();
        for (index, line) in self.text.split('\n').enumerate() {
            if index == cursor.line {
                at = start + grapheme_offset(line, cursor.column);
                break;
            }
            start += line.len() + 1;
        }
        self.at = at;
        self.want = None;
    }

    /// The buffer wrapped at `width` display columns: at least one row per
    /// line, more when a line is wider than the box. A width of zero
    /// counts as one column so every grapheme still lands somewhere.
    #[must_use]
    pub fn rows(&self, width: usize) -> Vec<Row> {
        let width = width.max(1);
        let mut rows = Vec::new();
        let mut line_start = 0;
        for (line, text) in self.lines().enumerate() {
            let mut start = 0;
            let mut used = 0;
            for (offset, grapheme) in text.grapheme_indices(true) {
                let cells = grapheme.width().max(1);
                if used + cells > width && offset > start {
                    rows.push(Row {
                        line,
                        start: line_start + start,
                        end: line_start + offset,
                    });
                    start = offset;
                    used = 0;
                }
                used += cells;
            }
            rows.push(Row {
                line,
                start: line_start + start,
                end: line_start + text.len(),
            });
            line_start += text.len() + 1;
        }
        rows
    }

    /// The text a row shows.
    #[must_use]
    pub fn row_text(&self, row: Row) -> &str {
        &self.text[row.start..row.end]
    }

    /// The wrapped row and display column the cursor is on at `width`.
    #[must_use]
    pub fn cursor_cell(&self, width: usize) -> Cell {
        let rows = self.rows(width);
        let index = rows
            .iter()
            .rposition(|row| row.start <= self.at && self.at <= row.end)
            .unwrap_or_default();
        let row = rows.get(index).copied().unwrap_or(Row {
            line: 0,
            start: 0,
            end: 0,
        });
        let column = self.text[row.start..self.at.max(row.start)].width();
        Cell { row: index, column }
    }

    /// Put the cursor on the grapheme at a wrapped cell (a mouse click),
    /// clamping to the end of the row or of the text.
    pub fn place_cursor(&mut self, width: usize, cell: Cell) {
        let rows = self.rows(width);
        let Some(row) = rows.get(cell.row).or(rows.last()).copied() else {
            return;
        };
        let text = &self.text[row.start..row.end];
        let mut used = 0;
        let mut offset = text.len();
        for (index, grapheme) in text.grapheme_indices(true) {
            if used >= cell.column {
                offset = index;
                break;
            }
            used += grapheme.width().max(1);
        }
        self.at = row.start + offset;
        self.want = None;
    }

    fn step(&mut self, motion: Motion) {
        let want = self.want.take();
        self.at = match motion {
            Motion::Left => self.prev_grapheme(),
            Motion::Right => self.next_grapheme(),
            Motion::LineStart => self.line_start(),
            Motion::LineEnd => self.line_end(),
            Motion::WordBack => self.word_back(),
            Motion::WordForward => self.word_forward(),
            Motion::Up | Motion::Down => {
                let Cursor { line, column } = self.cursor();
                let column = want.unwrap_or(column);
                let target = match motion {
                    Motion::Up => line.checked_sub(1),
                    _ => line
                        .checked_add(1)
                        .filter(|&next| next < self.lines().count()),
                };
                match target {
                    Some(target) => {
                        self.set_cursor(Cursor {
                            line: target,
                            column,
                        });
                        self.want = Some(column);
                        return;
                    }
                    // Off the top or bottom: to the very start or end,
                    // as chat inputs and readline do.
                    None if motion == Motion::Up => 0,
                    None => self.text.len(),
                }
            }
        };
    }

    fn remove(&mut self, range: std::ops::Range<usize>) {
        if range.is_empty() {
            return;
        }
        self.text.replace_range(range.clone(), "");
        self.at = range.start;
        self.want = None;
    }

    fn line_start(&self) -> usize {
        self.text[..self.at].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        self.text[self.at..]
            .find('\n')
            .map_or(self.text.len(), |i| self.at + i)
    }

    fn prev_grapheme(&self) -> usize {
        self.text[..self.at]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_grapheme(&self) -> usize {
        self.text[self.at..]
            .graphemes(true)
            .next()
            .map_or(self.text.len(), |g| self.at + g.len())
    }

    fn word_back(&self) -> usize {
        let before = &self.text[..self.at];
        let trimmed = before.trim_end();
        trimmed.rfind(char::is_whitespace).map_or(0, |i| {
            i + before[i..].chars().next().map_or(1, char::len_utf8)
        })
    }

    fn word_forward(&self) -> usize {
        let after = &self.text[self.at..];
        let skipped = after.len() - after.trim_start().len();
        let rest = &after[skipped..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        self.at + skipped + end
    }
}

/// Byte offset of grapheme `column` in `line`, or the line's end.
fn grapheme_offset(line: &str, column: usize) -> usize {
    line.grapheme_indices(true)
        .nth(column)
        .map_or(line.len(), |(i, _)| i)
}
