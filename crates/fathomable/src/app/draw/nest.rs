// @okf-doc: /decisions/0077-threads-nest-under-their-file.md
//! The nest (ADR 0077): the cells a thread's rows sit in from the
//! column's edge under the file row over them, in the review list and
//! the threads pane alike, so the threads read as the file's children
//! as the files pane nests a directory's.
//!
//! The file row does not move; every row of a thread does, by [`NEST`]
//! cells: in the list after the cursor cell, in the pane before the
//! circle. File scope drops the file rows and keeps the nest, so the
//! rows do not reflow and the chevron's click column stays put.

use ratatui::text::Span;

/// Cells a thread's rows sit in under its file row: one level, as the
/// files pane nests a directory's children, so a thread's chevron or
/// circle sits under the first letter of the path.
pub(crate) const NEST: usize = 2;

/// The nest's blank cells, drawn on a thread's row where the file row
/// has its chevron.
pub(crate) fn nest_span<'a>() -> Span<'a> {
    Span::raw(" ".repeat(NEST))
}
