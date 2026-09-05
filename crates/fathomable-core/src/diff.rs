// @okf-doc: /decisions/0006-git-access.md
//! Line diffs between two texts: hunks, per-line gutter status, and a
//! unified listing.
//!
//! [`Diff::new`] compares an old text (the HEAD blob, ADR 0006) with the
//! working-tree text using Myers' algorithm in linear space. The result
//! answers the two questions the viewer asks: which status the gutter bar
//! shows for a line of the new text ([`Diff::status`]), and what a unified
//! diff view lists ([`Diff::unified`]). Hunk starts drive `]g` / `[g`.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::diff::{Diff, LineStatus};
//!
//! let diff = Diff::new("a\nb\nc\n", "a\nB\nc\nd\n");
//! assert_eq!(diff.hunks().len(), 2);
//! assert_eq!(diff.status(2), Some(LineStatus::Modified));
//! assert_eq!(diff.status(4), Some(LineStatus::Added));
//! assert_eq!(diff.status(1), None);
//! ```

use std::fmt;
use std::ops::Range;

/// How a line of the new text differs from the old text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineStatus {
    /// The line is not in the old text.
    Added,
    /// The line replaces one or more old lines.
    Modified,
    /// Old lines were removed just above this line (ADR 0010 marks the
    /// line after a removal; at the end of the text, the last line).
    Removed,
}

/// One contiguous change: old lines replaced by new lines.
///
/// Either range may be empty: a pure insertion has no old lines and a pure
/// deletion no new ones.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hunk {
    old: Range<usize>,
    new: Range<usize>,
}

impl Hunk {
    /// The 0-based old lines the hunk removes; empty for a pure insertion.
    #[must_use]
    pub fn old_range(&self) -> Range<usize> {
        self.old.clone()
    }

    /// The 0-based new lines the hunk adds; empty for a pure removal, in
    /// which case `start` is the line the removal sits before.
    #[must_use]
    pub fn new_range(&self) -> Range<usize> {
        self.new.clone()
    }

    /// The 1-based new line to put the cursor on when jumping to the hunk.
    ///
    /// A pure removal has no line of its own, so the line after it is the
    /// target; `new_lines` caps that at the last line of the text.
    #[must_use]
    pub fn target_line(&self, new_lines: usize) -> usize {
        (self.new.start + 1).min(new_lines.max(1))
    }
}

impl fmt::Display for Hunk {
    /// The unified-diff hunk header, `@@ -a,b +c,d @@`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "@@ -{} +{} @@",
            header_range(&self.old),
            header_range(&self.new)
        )
    }
}

/// `start,len` as git prints it: 1-based, and an empty range names the
/// line before it.
fn header_range(range: &Range<usize>) -> String {
    let len = range.len();
    let start = if len == 0 {
        range.start
    } else {
        range.start + 1
    };
    if len == 1 {
        start.to_string()
    } else {
        format!("{start},{len}")
    }
}

/// What a line of a unified listing is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffKind {
    /// A hunk header.
    Header,
    /// A line present in both texts.
    Context,
    /// A line only in the new text.
    Added,
    /// A line only in the old text.
    Removed,
}

/// One line of a unified listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    kind: DiffKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
    text: String,
}

impl DiffLine {
    /// What the line is.
    #[must_use]
    pub fn kind(&self) -> DiffKind {
        self.kind
    }

    /// The 1-based old line, for context and removed lines.
    #[must_use]
    pub fn old_line(&self) -> Option<usize> {
        self.old_line
    }

    /// The 1-based new line, for context and added lines.
    #[must_use]
    pub fn new_line(&self) -> Option<usize> {
        self.new_line
    }

    /// The line text without its newline, or the header for
    /// [`DiffKind::Header`].
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// The difference between two texts, line by line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    hunks: Vec<Hunk>,
    old_lines: usize,
    new_lines: usize,
}

impl Diff {
    /// Compare `old` with `new`.
    ///
    /// Lines are compared exactly; a trailing newline does not start an
    /// extra line.
    #[must_use]
    pub fn new(old: &str, new: &str) -> Self {
        let a: Vec<&str> = old.lines().collect();
        let b: Vec<&str> = new.lines().collect();
        let mut hunks = Vec::new();
        myers(&a, &b, 0, 0, &mut hunks);
        Self {
            hunks,
            old_lines: a.len(),
            new_lines: b.len(),
        }
    }

    /// The changes, top to bottom.
    #[must_use]
    pub fn hunks(&self) -> &[Hunk] {
        &self.hunks
    }

    /// Whether the texts are line-for-line identical.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty()
    }

    /// Number of lines in the old text.
    #[must_use]
    pub fn old_lines(&self) -> usize {
        self.old_lines
    }

    /// Number of lines in the new text.
    #[must_use]
    pub fn new_lines(&self) -> usize {
        self.new_lines
    }

    /// The gutter status of 1-based new line `line`, if it changed.
    #[must_use]
    pub fn status(&self, line: usize) -> Option<LineStatus> {
        if line == 0 || line > self.new_lines {
            return None;
        }
        let index = line - 1;
        let mut removed = false;
        for hunk in &self.hunks {
            if hunk.new.contains(&index) {
                return Some(if hunk.old.is_empty() {
                    LineStatus::Added
                } else {
                    LineStatus::Modified
                });
            }
            if hunk.new.is_empty() && hunk.new.start.min(self.new_lines.saturating_sub(1)) == index
            {
                removed = true;
            }
        }
        removed.then_some(LineStatus::Removed)
    }

    /// `(added, removed)` line counts over every hunk.
    #[must_use]
    pub fn counts(&self) -> (usize, usize) {
        self.hunks.iter().fold((0, 0), |(added, removed), hunk| {
            (added + hunk.new.len(), removed + hunk.old.len())
        })
    }

    /// The hunk after 1-based new line `line`, wrapping to the first.
    ///
    /// Returns the hunk and whether the search wrapped; `None` when there
    /// are no hunks.
    #[must_use]
    pub fn next_hunk(&self, line: usize) -> Option<(&Hunk, bool)> {
        let after = self
            .hunks
            .iter()
            .find(|hunk| hunk.target_line(self.new_lines) > line);
        after
            .map(|hunk| (hunk, false))
            .or_else(|| self.hunks.first().map(|hunk| (hunk, true)))
    }

    /// The hunk before 1-based new line `line`, wrapping to the last.
    #[must_use]
    pub fn prev_hunk(&self, line: usize) -> Option<(&Hunk, bool)> {
        let before = self
            .hunks
            .iter()
            .rev()
            .find(|hunk| hunk.target_line(self.new_lines) < line);
        before
            .map(|hunk| (hunk, false))
            .or_else(|| self.hunks.last().map(|hunk| (hunk, true)))
    }

    /// A unified listing with `context` unchanged lines around each hunk.
    ///
    /// `old` and `new` must be the texts the diff was built from. Hunks
    /// whose context overlaps are merged under one header, as `git diff`
    /// does.
    #[must_use]
    pub fn unified(&self, old: &str, new: &str, context: usize) -> Vec<DiffLine> {
        let a: Vec<&str> = old.lines().collect();
        let b: Vec<&str> = new.lines().collect();
        let mut out = Vec::new();
        let mut groups: Vec<Vec<&Hunk>> = Vec::new();
        for hunk in &self.hunks {
            match groups.last_mut() {
                Some(group)
                    if group
                        .last()
                        .is_some_and(|last| hunk.old.start <= last.old.end + 2 * context) =>
                {
                    group.push(hunk);
                }
                _ => groups.push(vec![hunk]),
            }
        }
        for group in groups {
            let (Some(first), Some(last)) = (group.first(), group.last()) else {
                continue;
            };
            let old_from = first.old.start.saturating_sub(context);
            let old_to = (last.old.end + context).min(a.len());
            let new_from = first.new.start.saturating_sub(context);
            let new_to = (last.new.end + context).min(b.len());
            out.push(DiffLine {
                kind: DiffKind::Header,
                old_line: None,
                new_line: None,
                text: Hunk {
                    old: old_from..old_to,
                    new: new_from..new_to,
                }
                .to_string(),
            });
            let (mut i, mut j) = (old_from, new_from);
            for hunk in group {
                while i < hunk.old.start {
                    out.push(context_line(&a, &b, i, j));
                    i += 1;
                    j += 1;
                }
                for k in hunk.old.clone() {
                    out.push(DiffLine {
                        kind: DiffKind::Removed,
                        old_line: Some(k + 1),
                        new_line: None,
                        text: a.get(k).copied().unwrap_or_default().to_owned(),
                    });
                }
                for k in hunk.new.clone() {
                    out.push(DiffLine {
                        kind: DiffKind::Added,
                        old_line: None,
                        new_line: Some(k + 1),
                        text: b.get(k).copied().unwrap_or_default().to_owned(),
                    });
                }
                i = hunk.old.end;
                j = hunk.new.end;
            }
            while i < old_to && j < new_to {
                out.push(context_line(&a, &b, i, j));
                i += 1;
                j += 1;
            }
        }
        out
    }
}

fn context_line(a: &[&str], b: &[&str], i: usize, j: usize) -> DiffLine {
    DiffLine {
        kind: DiffKind::Context,
        old_line: Some(i + 1),
        new_line: Some(j + 1),
        text: b
            .get(j)
            .or_else(|| a.get(i))
            .copied()
            .unwrap_or_default()
            .to_owned(),
    }
}

/// Append the hunks between `a` and `b` to `out`; `a0` and `b0` are the
/// absolute line numbers of `a[0]` and `b[0]`.
///
/// Linear-space Myers: strip the common prefix and suffix, find the middle
/// snake with a forward and a reverse search, and recurse on both halves.
fn myers(a: &[&str], b: &[&str], a0: usize, b0: usize, out: &mut Vec<Hunk>) {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a, b) = (&a[prefix..], &b[prefix..]);
    let (a0, b0) = (a0 + prefix, b0 + prefix);
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a, b) = (&a[..a.len() - suffix], &b[..b.len() - suffix]);
    if a.is_empty() && b.is_empty() {
        return;
    }
    if a.is_empty() || b.is_empty() {
        push_hunk(out, a0..a0 + a.len(), b0..b0 + b.len());
        return;
    }
    let (x, y) = middle(a, b);
    if (x == 0 && y == 0) || (x == a.len() && y == b.len()) {
        // No progress would be made; treat the rest as one replacement.
        push_hunk(out, a0..a0 + a.len(), b0..b0 + b.len());
        return;
    }
    myers(&a[..x], &b[..y], a0, b0, out);
    myers(&a[x..], &b[y..], a0 + x, b0 + y, out);
}

/// Record a hunk, merging it into the previous one when they touch.
fn push_hunk(out: &mut Vec<Hunk>, old: Range<usize>, new: Range<usize>) {
    if let Some(last) = out.last_mut()
        && last.old.end == old.start
        && last.new.end == new.start
    {
        last.old.end = old.end;
        last.new.end = new.end;
        return;
    }
    out.push(Hunk { old, new });
}

/// The point `(x, y)` at which the shortest edit path crosses the middle,
/// found by running the forward and reverse searches until they meet.
///
/// Both `a` and `b` are non-empty and share no common prefix or suffix.
fn middle(a: &[&str], b: &[&str]) -> (usize, usize) {
    let n = to_isize(a.len());
    let m = to_isize(b.len());
    let max_d = (n + m + 1) / 2;
    let offset = max_d;
    let size = to_usize(2 * max_d + 2);
    let mut forward = vec![-1isize; size];
    let mut reverse = vec![-1isize; size];
    forward[to_usize(offset + 1)] = 0;
    reverse[to_usize(offset + 1)] = 0;
    let delta = n - m;
    let front = delta % 2 != 0;
    let (mut k1_start, mut k1_end, mut k2_start, mut k2_end) = (0, 0, 0, 0);
    for d in 0..=max_d {
        let mut k1 = -d + k1_start;
        while k1 <= d - k1_end {
            let at = to_usize(offset + k1);
            let mut x1 = if k1 == -d || (k1 != d && forward[at - 1] < forward[at + 1]) {
                forward[at + 1]
            } else {
                forward[at - 1] + 1
            };
            let mut y1 = x1 - k1;
            while x1 < n && y1 < m && a[to_usize(x1)] == b[to_usize(y1)] {
                x1 += 1;
                y1 += 1;
            }
            forward[at] = x1;
            if x1 > n {
                k1_end += 2;
            } else if y1 > m {
                k1_start += 2;
            } else if front {
                let k2 = offset + delta - k1;
                if k2 >= 0 && k2 < to_isize(size) && reverse[to_usize(k2)] != -1 {
                    let x2 = n - reverse[to_usize(k2)];
                    if x1 >= x2 {
                        return (to_usize(x1), to_usize(y1));
                    }
                }
            }
            k1 += 2;
        }
        let mut k2 = -d + k2_start;
        while k2 <= d - k2_end {
            let at = to_usize(offset + k2);
            let mut x2 = if k2 == -d || (k2 != d && reverse[at - 1] < reverse[at + 1]) {
                reverse[at + 1]
            } else {
                reverse[at - 1] + 1
            };
            let mut y2 = x2 - k2;
            while x2 < n && y2 < m && a[to_usize(n - x2 - 1)] == b[to_usize(m - y2 - 1)] {
                x2 += 1;
                y2 += 1;
            }
            reverse[at] = x2;
            if x2 > n {
                k2_end += 2;
            } else if y2 > m {
                k2_start += 2;
            } else if !front {
                let k1 = offset + delta - k2;
                if k1 >= 0 && k1 < to_isize(size) && forward[to_usize(k1)] != -1 {
                    let x1 = forward[to_usize(k1)];
                    let y1 = offset + x1 - k1;
                    if x1 >= n - x2 {
                        return (to_usize(x1), to_usize(y1));
                    }
                }
            }
            k2 += 2;
        }
    }
    // Unreachable for non-empty inputs: the searches always meet by
    // d = max_d. Fall back to "replace everything".
    (a.len(), b.len())
}

fn to_isize(value: usize) -> isize {
    isize::try_from(value).unwrap_or(isize::MAX)
}

fn to_usize(value: isize) -> usize {
    usize::try_from(value).unwrap_or(0)
}
