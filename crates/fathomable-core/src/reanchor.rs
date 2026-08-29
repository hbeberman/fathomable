// @okf-doc: /decisions/0019-reanchoring-edited-lines.md
//! Re-anchoring a thread whose lines were edited (ADR 0019).
//!
//! [`Anchor::locate`](crate::annotations::Anchor::locate) finds lines that
//! still hash the same. When an annotated line was *edited*, the hashes
//! are gone but the line is not: [`map_range`] follows the range through
//! the line diff between the text the thread was last placed in and the
//! current text, so the viewer can re-anchor it to the replacement lines
//! and say so, instead of detaching it.

use crate::annotations::LineRange;
use crate::diff::Diff;

/// Where a range of the old text landed in the new text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mapping {
    /// No annotated line was touched; the range only moved.
    Moved(LineRange),
    /// At least one annotated line was replaced; this is where the
    /// replacement sits.
    Edited(LineRange),
    /// Every annotated line was removed without replacement.
    Removed,
}

/// Lines of context on each side of a range that an edit may reach and
/// still count as an edit *of* the range rather than a rewrite around it.
pub(crate) const LOCAL_CONTEXT: usize = 1;

/// Follow `range` (1-based lines of `old`) into `new` through a line diff.
///
/// A line outside every hunk keeps its position shifted by the hunks above
/// it. A line inside a hunk lands at the same offset within the hunk's
/// replacement, clamped to its last line; a hunk with no replacement
/// removes the line. The result spans the surviving lines.
///
/// Only a local edit is followed: a hunk that reaches more than one line
/// beyond the range on either side is a rewrite of the surroundings, and
/// the lines it swallows count as removed, so a wholesale rewrite detaches
/// the thread instead of pinning it to an unrelated replacement.
#[must_use]
pub fn map_range(old: &str, new: &str, range: LineRange) -> Mapping {
    let diff = Diff::new(old, new);
    let local = (range.start() - 1).saturating_sub(LOCAL_CONTEXT)..range.end() + LOCAL_CONTEXT;
    let mut edited = false;
    let mut lowest = usize::MAX;
    let mut highest = 0;
    for old_line in (range.start() - 1)..range.end().min(diff.old_lines()) {
        let mut shift: isize = 0;
        let mut landed = Some(old_line);
        for hunk in diff.hunks() {
            let (from, to) = (hunk.old_range(), hunk.new_range());
            if from.contains(&old_line) {
                edited = true;
                let is_local = from.start >= local.start && from.end <= local.end;
                landed = (is_local && !to.is_empty())
                    .then(|| (to.start + old_line - from.start).min(to.end - 1));
                shift = 0;
                break;
            }
            if from.start > old_line {
                break;
            }
            shift +=
                isize::try_from(to.len()).unwrap_or(0) - isize::try_from(from.len()).unwrap_or(0);
        }
        if let Some(new_line) = landed.and_then(|line| line.checked_add_signed(shift)) {
            lowest = lowest.min(new_line);
            highest = highest.max(new_line);
        }
    }
    if lowest == usize::MAX {
        return Mapping::Removed;
    }
    let range = LineRange::new(lowest + 1, highest + 1);
    if edited {
        Mapping::Edited(range)
    } else {
        Mapping::Moved(range)
    }
}
