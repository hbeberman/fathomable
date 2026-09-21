// @okf-doc: /decisions/0090-direct-workspace-navigation.md
//! Pure viewport placement for deliberate navigation.

use std::ops::Range;

/// Center `span` when it fits, otherwise center the start of `priority`.
pub(crate) fn center_span(
    span: Range<usize>,
    priority: Range<usize>,
    body_height: usize,
    max_scroll: usize,
) -> usize {
    let height = body_height.max(1);
    let anchor = if span.len() <= height {
        span.start
            .saturating_sub((height.saturating_sub(span.len())) / 2)
    } else {
        priority
            .start
            .saturating_sub((height.saturating_sub(1)) / 2)
    };
    anchor.min(max_scroll)
}

/// Place `row` at `floor((body_height - 1) / 3)`.
pub(crate) fn top_third(row: usize, body_height: usize, max_scroll: usize) -> usize {
    row.saturating_sub(body_height.saturating_sub(1) / 3)
        .min(max_scroll)
}

#[cfg(test)]
mod tests {
    use super::{center_span, top_third};

    #[test]
    fn fitting_span_is_centered_with_even_height_rounding_up_below() {
        assert_eq!(center_span(10..14, 13..14, 10, 100), 7);
        assert_eq!(center_span(10..15, 14..15, 10, 100), 8);
    }

    #[test]
    fn overflowing_span_centers_priority_start_and_clamps() {
        assert_eq!(center_span(2..30, 20..25, 10, 100), 16);
        assert_eq!(center_span(0..30, 2..3, 10, 100), 0);
        assert_eq!(center_span(90..120, 115..120, 10, 108), 108);
    }

    #[test]
    fn top_third_uses_exact_floor_semantics() {
        assert_eq!(top_third(10, 1, 100), 10);
        assert_eq!(top_third(10, 8, 100), 8);
        assert_eq!(top_third(10, 9, 100), 8);
        assert_eq!(top_third(2, 9, 100), 0);
        assert_eq!(top_third(20, 9, 11), 11);
    }
}
