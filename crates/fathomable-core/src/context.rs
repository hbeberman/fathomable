// @okf-doc: /decisions/0038-reanchoring-without-a-snapshot.md
//! A thread's context window: the text it was last placed in, carried
//! with the thread so an offline edit can be projected without storing a
//! reader snapshot (ADR 0038).
//!
//! [`Context::capture`] takes the annotated lines and up to
//! [`CONTEXT_LINES`] lines either side. [`map_context`] finds the window's
//! outer lines in the current text, cuts the region between them, and
//! follows the annotated lines through the diff of window against region
//! with [`map_range`].
//!
//! # Examples
//!
//! ```
//! use fathomable_core::annotations::LineRange;
//! use fathomable_core::context::{Context, map_context};
//! use fathomable_core::reanchor::Mapping;
//!
//! let old = "a\nb\nc\nd\ne\nf\ng\n";
//! let context = Context::capture(old, LineRange::new(4, 4)).unwrap();
//! let new = "x\ny\na\nb\nc\nD\ne\nf\ng\n";
//! assert_eq!(
//!     map_context(&context, new, LineRange::new(4, 4)),
//!     Mapping::Edited(LineRange::new(6, 6))
//! );
//! ```

use serde::{Deserialize, Serialize};

use crate::annotations::{LineRange, line_hash};
use crate::reanchor::{LOCAL_CONTEXT, Mapping, map_range};

/// Lines kept on each side of the annotated range.
pub const CONTEXT_LINES: usize = 3;

/// Maximum bytes retained by a context window.
pub const MAX_CONTEXT_BYTES: usize = 16 * 1024;

/// The annotated lines of a thread with the lines around them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Context {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    before: Vec<String>,
    lines: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    after: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
}

impl Context {
    /// Take `range` of `text` and up to [`CONTEXT_LINES`] lines either
    /// side; `None` when the range runs past the end of the text.
    #[must_use]
    pub fn capture(text: &str, range: LineRange) -> Option<Self> {
        Self::capture_bounded(text, range, MAX_CONTEXT_BYTES)
    }

    /// Capture a context window with an explicit byte bound.
    #[must_use]
    pub fn capture_bounded(text: &str, range: LineRange, max_bytes: usize) -> Option<Self> {
        let lines: Vec<&str> = text.lines().collect();
        if range.end() > lines.len() {
            return None;
        }
        let owned = |slice: &[&str]| slice.iter().map(|line| (*line).to_owned()).collect();
        let start = range.start() - 1;
        let mut context = Self {
            before: owned(&lines[start.saturating_sub(CONTEXT_LINES)..start]),
            lines: owned(&lines[start..range.end()]),
            after: owned(&lines[range.end()..(range.end() + CONTEXT_LINES).min(lines.len())]),
            truncated: false,
        };
        context.bound_to(max_bytes);
        Some(context)
    }

    fn bound_to(&mut self, max_bytes: usize) {
        while self.text().len() > max_bytes && !self.before.is_empty() {
            self.before.remove(0);
            self.truncated = true;
        }
        while self.text().len() > max_bytes && !self.after.is_empty() {
            self.after.pop();
            self.truncated = true;
        }
        if self.text().len() <= max_bytes {
            return;
        }
        let mut remaining = max_bytes;
        for line in self
            .before
            .iter_mut()
            .chain(self.lines.iter_mut())
            .chain(self.after.iter_mut())
        {
            if remaining == 0 {
                line.clear();
                self.truncated = true;
                continue;
            }
            let budget = remaining.saturating_sub(1);
            if line.len() > budget {
                let mut end = budget.min(line.len());
                while end > 0 && !line.is_char_boundary(end) {
                    end -= 1;
                }
                line.truncate(end);
                self.truncated = true;
            }
            remaining = remaining.saturating_sub(line.len() + 1);
        }
    }

    /// The window as one text, lines joined by newlines.
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = String::new();
        for line in self.before.iter().chain(&self.lines).chain(&self.after) {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// Where the annotated lines sit within [`Self::text`].
    #[must_use]
    pub fn range(&self) -> LineRange {
        let start = self.before.len() + 1;
        LineRange::new(start, start + self.lines.len().max(1) - 1)
    }

    /// The annotated lines, without the trailing newline.
    #[must_use]
    pub fn snippet(&self) -> String {
        self.lines.join("\n")
    }

    /// Whether the context was shortened to satisfy its byte bound.
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }
}

/// Follow the annotated lines of `context` into `current` (ADR 0038).
///
/// The outer lines of each side of the window, those beyond the one line
/// of slack an edit may reach, are found in `current` as whole blocks by
/// line hash, the match nearest `hint` winning; an empty side stands for
/// the file's edge. Both must be found, in order. The window is then
/// diffed against the lines between the blocks with `map_range`, and the
/// result shifted into `current`. A side that cannot be found reads as a
/// rewrite of the surroundings: `Mapping::Removed`.
#[must_use]
pub fn map_context(context: &Context, current: &str, hint: LineRange) -> Mapping {
    let hashes: Vec<String> = current.lines().map(line_hash).collect();
    let outer_before = &context.before[..context.before.len().saturating_sub(LOCAL_CONTEXT)];
    let outer_after = &context.after[LOCAL_CONTEXT.min(context.after.len())..];
    let Some(region_start) = locate_block(&hashes, outer_before, hint.start(), 0) else {
        return Mapping::Removed;
    };
    let after_from = region_start + outer_before.len();
    let region_end = if context.after.len() > LOCAL_CONTEXT {
        match locate_block(&hashes, outer_after, hint.end(), after_from) {
            Some(start) => start + outer_after.len(),
            None => return Mapping::Removed,
        }
    } else {
        // The window met the end of the file; the region runs to it.
        hashes.len()
    };
    let region: String = current
        .lines()
        .skip(region_start)
        .take(region_end - region_start)
        .flat_map(|line| [line, "\n"])
        .collect();
    match map_range(&context.text(), &region, context.range()) {
        Mapping::Moved(range) => Mapping::Moved(shift(range, region_start)),
        Mapping::Edited(range) => Mapping::Edited(shift(range, region_start)),
        Mapping::Removed => Mapping::Removed,
    }
}

/// The 0-based start of the occurrence of `block` in `hashes` at or after
/// `from` nearest the 1-based `hint`. An empty block is the file's start
/// (or `from`), where it stands for the file's edge.
fn locate_block(hashes: &[String], block: &[String], hint: usize, from: usize) -> Option<usize> {
    if block.is_empty() {
        return (from == 0).then_some(0);
    }
    let want: Vec<String> = block.iter().map(|line| line_hash(line)).collect();
    hashes
        .get(from..)?
        .windows(want.len())
        .enumerate()
        .filter(|(_, window)| *window == want.as_slice())
        .map(|(offset, _)| from + offset)
        .min_by_key(|start| (start + 1).abs_diff(hint))
}

fn shift(range: LineRange, by: usize) -> LineRange {
    LineRange::new(range.start() + by, range.end() + by)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";

    type Result = std::result::Result<(), &'static str>;

    fn window(range: LineRange) -> std::result::Result<Context, &'static str> {
        Context::capture(OLD, range).ok_or("range in text")
    }

    #[test]
    fn capture_takes_three_lines_either_side_or_fewer_at_the_edges() -> Result {
        let mid = window(LineRange::new(5, 5))?;
        assert_eq!(mid.text(), "two\nthree\nfour\nfive\nsix\nseven\neight\n");
        assert_eq!(mid.range(), LineRange::new(4, 4));
        assert_eq!(mid.snippet(), "five");
        let top = window(LineRange::new(1, 2))?;
        assert_eq!(top.text(), "one\ntwo\nthree\nfour\nfive\n");
        assert_eq!(top.range(), LineRange::new(1, 2));
        let bottom = window(LineRange::new(9, 9))?;
        assert_eq!(bottom.text(), "six\nseven\neight\nnine\n");
        assert_eq!(bottom.range(), LineRange::new(4, 4));
        assert!(Context::capture(OLD, LineRange::new(9, 10)).is_none());

        Ok(())
    }

    #[test]
    fn an_edit_inside_the_window_is_followed_past_inserted_lines() -> Result {
        let context = window(LineRange::new(5, 5))?;
        let new = "a\nb\nc\none\ntwo\nthree\nfour\nFIVE\nsix\nseven\neight\nnine\n";
        assert_eq!(
            map_context(&context, new, LineRange::new(5, 5)),
            Mapping::Edited(LineRange::new(8, 8))
        );

        Ok(())
    }

    #[test]
    fn untouched_lines_only_move() -> Result {
        let context = window(LineRange::new(5, 6))?;
        let new = "zero\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\n";
        assert_eq!(
            map_context(&context, new, LineRange::new(5, 6)),
            Mapping::Moved(LineRange::new(6, 7))
        );

        Ok(())
    }

    #[test]
    fn an_edit_reaching_one_line_out_is_local_but_two_is_a_rewrite() -> Result {
        let context = window(LineRange::new(5, 5))?;
        let one = "one\ntwo\nthree\nFOUR\nFIVE\nsix\nseven\neight\nnine\n";
        assert_eq!(
            map_context(&context, one, LineRange::new(5, 5)),
            Mapping::Edited(LineRange::new(5, 5))
        );
        let two = "one\ntwo\nTHREE\nFOUR\nFIVE\nsix\nseven\neight\nnine\n";
        assert_eq!(
            map_context(&context, two, LineRange::new(5, 5)),
            Mapping::Removed
        );

        Ok(())
    }

    #[test]
    fn removed_lines_and_missing_surroundings_detach() -> Result {
        let context = window(LineRange::new(5, 5))?;
        let removed = "one\ntwo\nthree\nfour\nsix\nseven\neight\nnine\n";
        assert_eq!(
            map_context(&context, removed, LineRange::new(5, 5)),
            Mapping::Removed
        );
        assert_eq!(
            map_context(&context, "other\n", LineRange::new(5, 5)),
            Mapping::Removed
        );

        Ok(())
    }

    #[test]
    fn edges_of_the_file_stand_in_for_missing_context() -> Result {
        let top = window(LineRange::new(1, 1))?;
        assert_eq!(
            map_context(&top, "ONE\ntwo\nthree\nfour\nfive\n", LineRange::new(1, 1)),
            Mapping::Edited(LineRange::new(1, 1))
        );
        let bottom = window(LineRange::new(9, 9))?;
        assert_eq!(
            map_context(
                &bottom,
                "six\nseven\neight\nNINE\nten\n",
                LineRange::new(9, 9)
            ),
            Mapping::Edited(LineRange::new(4, 4))
        );

        Ok(())
    }

    #[test]
    fn the_match_nearest_the_hint_wins_when_context_repeats() -> Result {
        let old = "x\ny\nz\na\nb\nc\nx\ny\nz\nd\nb\nc\n";
        let context = Context::capture(old, LineRange::new(10, 10)).ok_or("in text")?;
        let new = "x\ny\nz\na\nb\nc\nx\ny\nz\nD\nb\nc\n";
        assert_eq!(
            map_context(&context, new, LineRange::new(10, 10)),
            Mapping::Edited(LineRange::new(10, 10))
        );

        Ok(())
    }

    #[test]
    fn bounded_capture_marks_and_limits_large_evidence() -> Result {
        let text = "aaaaaaaaaa\nbbbbbbbbbb\ncccccccccc\ndddddddddd\n";
        let context =
            Context::capture_bounded(text, LineRange::new(2, 3), 12).ok_or("range in text")?;
        assert!(context.is_truncated());
        assert!(context.text().len() <= 12);
        assert_eq!(context.range().len(), 2);
        Ok(())
    }
}
