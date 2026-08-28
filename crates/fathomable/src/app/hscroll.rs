// @okf-doc: /decisions/0029-horizontal-scroll.md
//! Horizontal scroll for unwrapped lines (ADR 0029).
//!
//! A view keeps one [`HScroll`] column offset per open document. Only the
//! layout's unwrapped lines (code block lines, source and diff lines, rows
//! of a table wider than the pane) shift by it; the offset is clamped
//! against the widest of them so the view never scrolls into nothing.
//! [`key`] and [`mouse`] translate the `z` prefix keys and the horizontal
//! wheel into view operations; `ui` slices each row by the offset.

use crossterm::event::{KeyCode, MouseEventKind};
use fathomable_core::layout::Layout;

use super::view::View;

/// Columns the horizontal wheel scrolls per tick.
pub const WHEEL_COLUMNS: usize = 4;

/// Columns kept between a revealed search match and the pane edge.
const MARGIN: usize = 4;

/// The column offset of a document's unwrapped lines.
///
/// The stored value is never clamped, so an offset survives a relayout
/// (a reload, a diff or source toggle) whose widest line is briefly
/// narrower; [`Self::offset`] clamps on read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HScroll {
    offset: usize,
}

impl HScroll {
    /// The effective offset for `layout`: at most the widest unwrapped
    /// line minus one, and zero when no unwrapped line is wider than the
    /// pane.
    #[must_use]
    pub fn offset(self, layout: &Layout) -> usize {
        self.offset.min(Self::max_offset(layout))
    }

    /// Scroll by `delta` columns from the effective offset.
    pub fn scroll_by(&mut self, delta: isize, layout: &Layout) {
        self.offset = self
            .offset(layout)
            .saturating_add_signed(delta)
            .min(Self::max_offset(layout));
    }

    /// Move the least amount that shows shiftable column `col` of an
    /// unwrapped line with [`MARGIN`] cells to spare on its side of the
    /// pane, whose shiftable part is `avail` cells wide.
    pub fn reveal(&mut self, col: usize, avail: usize, layout: &Layout) {
        let offset = self.offset(layout);
        let margin = MARGIN.min(avail.saturating_sub(1) / 2);
        if col < offset + margin {
            self.offset = col.saturating_sub(margin);
        } else if col + margin >= offset + avail {
            self.offset = (col + margin + 1).saturating_sub(avail);
        } else {
            return;
        }
        self.offset = self.offset.min(Self::max_offset(layout));
    }

    fn max_offset(layout: &Layout) -> usize {
        let widest = layout.unwrapped_width();
        if widest > layout.width() {
            widest - 1
        } else {
            0
        }
    }
}

/// The key after the `z` prefix in the view: `l`/`h` scroll `count`
/// columns, `L`/`H` half the text width. Returns whether the key was one
/// of them.
pub fn key(view: &mut View, code: KeyCode, count: usize) -> bool {
    let half = (view.layout().width() / 2).max(1);
    let columns = |n: usize| isize::try_from(n).unwrap_or(isize::MAX);
    let delta = match code {
        KeyCode::Char('l') | KeyCode::Right => columns(count.max(1)),
        KeyCode::Char('h') | KeyCode::Left => -columns(count.max(1)),
        KeyCode::Char('L') => columns(half),
        KeyCode::Char('H') => -columns(half),
        _ => return false,
    };
    view.scroll_columns(delta);
    true
}

/// The horizontal wheel over the text pane: [`WHEEL_COLUMNS`] per tick.
/// Returns whether the event was a horizontal tick.
pub fn mouse(view: &mut View, kind: MouseEventKind) -> bool {
    let columns = isize::try_from(WHEEL_COLUMNS).unwrap_or(isize::MAX);
    match kind {
        MouseEventKind::ScrollRight => view.scroll_columns(columns),
        MouseEventKind::ScrollLeft => view.scroll_columns(-columns),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, MouseEventKind};

    use super::{HScroll, key, mouse};
    use crate::app::view::View;

    const CODE: &str = "```\nlet value = 0123456789_0123456789_0123456789;\n```\n\nprose\n";

    #[test]
    fn offset_clamps_to_the_widest_unwrapped_line() {
        let view = View::new(CODE.to_owned(), 20, 5);
        let layout = view.layout();
        let mut scroll = HScroll::default();
        scroll.scroll_by(1000, layout);
        assert_eq!(scroll.offset(layout), layout.unwrapped_width() - 1);
        scroll.scroll_by(-3, layout);
        assert_eq!(scroll.offset(layout), layout.unwrapped_width() - 4);
        scroll.scroll_by(-1000, layout);
        assert_eq!(scroll.offset(layout), 0);

        let fits = View::new("```\nshort\n```\n".to_owned(), 20, 5);
        let mut scroll = HScroll::default();
        scroll.scroll_by(5, fits.layout());
        assert_eq!(scroll.offset(fits.layout()), 0, "nothing to scroll into");
    }

    #[test]
    fn reveal_moves_the_least_with_a_margin() {
        let view = View::new(CODE.to_owned(), 20, 5);
        let layout = view.layout();
        let mut scroll = HScroll::default();
        scroll.reveal(3, 20, layout);
        assert_eq!(scroll.offset(layout), 0, "already in view");
        scroll.reveal(30, 20, layout);
        assert_eq!(
            scroll.offset(layout),
            15,
            "column 30 sits four from the right edge"
        );
        scroll.reveal(16, 20, layout);
        assert_eq!(
            scroll.offset(layout),
            12,
            "column 16 sits four from the left edge"
        );
    }

    #[test]
    fn z_keys_and_the_horizontal_wheel_scroll_the_view() {
        let mut view = View::new(CODE.to_owned(), 20, 5);
        assert!(key(&mut view, KeyCode::Char('l'), 0));
        assert_eq!(view.column_offset(), 1);
        assert!(key(&mut view, KeyCode::Char('l'), 10));
        assert_eq!(view.column_offset(), 11);
        assert!(key(&mut view, KeyCode::Char('h'), 5));
        assert_eq!(view.column_offset(), 6);
        assert!(key(&mut view, KeyCode::Char('L'), 0));
        assert_eq!(view.column_offset(), 16);
        assert!(key(&mut view, KeyCode::Char('H'), 0));
        assert_eq!(view.column_offset(), 6);
        assert!(!key(&mut view, KeyCode::Char('x'), 0));
        assert!(mouse(&mut view, MouseEventKind::ScrollRight));
        assert_eq!(view.column_offset(), 10);
        assert!(mouse(&mut view, MouseEventKind::ScrollLeft));
        assert_eq!(view.column_offset(), 6);
        assert!(!mouse(&mut view, MouseEventKind::ScrollDown));
    }
}
