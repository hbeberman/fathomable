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
//! Repository-wide endpoint deltas use [`Comparison`], [`PathChange`], and
//! [`PathState`]. They retain additions, deletions, content, mode, type,
//! binary, unsupported, and missing-content facts without exposing Git types.
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
use std::path::{Path, PathBuf};

use crate::workspace::ComparisonEndpoint;

/// How lines are compared (ADR 0060): exactly, or with whitespace
/// ignored as `git diff -w` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Whitespace {
    /// Lines match only when identical.
    #[default]
    Exact,
    /// Lines match when they are identical once every whitespace
    /// character is removed.
    Ignore,
}

/// How a diff is computed and listed (ADR 0060): the whitespace rule and
/// the unchanged lines shown around each hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Compare {
    /// Unchanged lines listed around each hunk; three by default.
    pub context: usize,
    /// The whitespace rule.
    pub whitespace: Whitespace,
}

/// The Git file mode represented by a comparison endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileMode {
    /// A non-executable regular file.
    Regular,
    /// An executable regular file.
    Executable,
    /// A symbolic link.
    Symlink,
    /// A directory entry.
    Directory,
    /// A Git submodule entry.
    Submodule,
    /// A mode not supported by the viewer.
    Other(u32),
}

impl FileMode {
    /// Whether this mode can be loaded as file content.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Regular | Self::Executable | Self::Symlink)
    }

    /// Whether this mode describes a directory-like entry.
    #[must_use]
    pub const fn is_directory(self) -> bool {
        matches!(self, Self::Directory)
    }

    fn type_tag(self) -> u8 {
        match self {
            Self::Regular | Self::Executable => 0,
            Self::Symlink => 1,
            Self::Directory => 2,
            Self::Submodule => 3,
            Self::Other(_) => 4,
        }
    }
}

/// Facts about one path on one comparison endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PathInfo {
    mode: FileMode,
    size: Option<u64>,
    object: Option<String>,
    binary: bool,
    supported: bool,
}

impl PathInfo {
    /// Creates endpoint facts for a path.
    #[must_use]
    pub(crate) fn new(
        mode: FileMode,
        size: Option<u64>,
        object: Option<String>,
        binary: bool,
    ) -> Self {
        Self {
            mode,
            size,
            object,
            binary,
            supported: mode.is_supported(),
        }
    }

    pub(crate) fn with_binary(mut self, binary: bool) -> Self {
        self.binary = binary;
        self
    }

    pub(crate) fn with_supported(mut self, supported: bool) -> Self {
        self.supported = supported && self.mode.is_supported();
        self
    }

    /// The Git file mode or endpoint type.
    #[must_use]
    pub const fn mode(&self) -> FileMode {
        self.mode
    }

    /// The byte size when the endpoint can report one.
    #[must_use]
    pub const fn size(&self) -> Option<u64> {
        self.size
    }

    /// The immutable object identity, when the endpoint has one.
    #[must_use]
    pub fn object(&self) -> Option<&str> {
        self.object.as_deref()
    }

    /// Whether the endpoint content is binary.
    #[must_use]
    pub const fn is_binary(&self) -> bool {
        self.binary
    }

    /// Whether the endpoint content can be loaded as file bytes.
    #[must_use]
    pub const fn is_supported(&self) -> bool {
        self.supported
    }
}

/// The state of one path on one side of a comparison.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PathState {
    /// The path is not present on this endpoint.
    Absent,
    /// The path is present with these facts.
    Present(PathInfo),
    /// The path exists, but its backing content could not be read.
    Missing(String),
}

/// The primary fact established for a changed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathChangeKind {
    /// The path exists only on the target endpoint.
    Added,
    /// The path exists only on the base endpoint.
    Deleted,
    /// File content differs.
    ContentChanged,
    /// File mode differs while its type remains the same.
    ModeChanged,
    /// The endpoint types differ.
    TypeChanged,
    /// The path is binary, so line-level content is not available.
    Binary,
    /// The path or its mode is unsupported.
    Unsupported,
    /// Endpoint content is missing or unavailable.
    Missing,
}

/// One repository-relative path in a comparison delta.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PathChange {
    path: PathBuf,
    base: PathState,
    target: PathState,
    kind: PathChangeKind,
    content_changed: bool,
    mode_changed: bool,
    type_changed: bool,
}

impl PathChange {
    /// The root-relative path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The base-side facts.
    #[must_use]
    pub const fn base(&self) -> &PathState {
        &self.base
    }

    /// The target-side facts.
    #[must_use]
    pub const fn target(&self) -> &PathState {
        &self.target
    }

    /// The primary classification of this path.
    #[must_use]
    pub const fn kind(&self) -> PathChangeKind {
        self.kind
    }

    /// Whether file bytes differ.
    #[must_use]
    pub const fn content_changed(&self) -> bool {
        self.content_changed
    }

    /// Whether file mode bits differ.
    #[must_use]
    pub const fn mode_changed(&self) -> bool {
        self.mode_changed
    }

    /// Whether endpoint file types differ.
    #[must_use]
    pub const fn type_changed(&self) -> bool {
        self.type_changed
    }

    pub(crate) fn from_states(
        path: PathBuf,
        base: PathState,
        target: PathState,
        base_bytes: Option<&[u8]>,
        target_bytes: Option<&[u8]>,
    ) -> Option<Self> {
        if matches!(base, PathState::Missing(_)) || matches!(target, PathState::Missing(_)) {
            return Some(Self {
                path,
                base,
                target,
                kind: PathChangeKind::Missing,
                content_changed: false,
                mode_changed: false,
                type_changed: false,
            });
        }
        let (base_info, target_info) = match (&base, &target) {
            (PathState::Present(base), PathState::Present(target)) => (base, target),
            (PathState::Absent, PathState::Present(target)) => {
                if target.mode.is_directory() {
                    return None;
                }
                return Some(Self {
                    path,
                    base: base.clone(),
                    target: PathState::Present(target.clone()),
                    kind: if target.is_supported() {
                        if target.is_binary() {
                            PathChangeKind::Binary
                        } else {
                            PathChangeKind::Added
                        }
                    } else {
                        PathChangeKind::Unsupported
                    },
                    content_changed: true,
                    mode_changed: false,
                    type_changed: false,
                });
            }
            (PathState::Present(base), PathState::Absent) => {
                if base.mode.is_directory() {
                    return None;
                }
                return Some(Self {
                    path,
                    base: PathState::Present(base.clone()),
                    target: target.clone(),
                    kind: PathChangeKind::Deleted,
                    content_changed: true,
                    mode_changed: false,
                    type_changed: false,
                });
            }
            (PathState::Absent, PathState::Absent) => return None,
            _ => unreachable!("missing path states were handled above"),
        };
        if base_info.mode.is_directory() && target_info.mode.is_directory() {
            return None;
        }

        let type_changed = base_info.mode.type_tag() != target_info.mode.type_tag();
        let mode_changed = base_info.mode != target_info.mode;
        let content_changed = match (base_bytes, target_bytes) {
            (Some(base), Some(target)) => base != target,
            _ => base_info.object != target_info.object,
        };
        if !type_changed && !mode_changed && !content_changed {
            return None;
        }
        let kind = if !base_info.is_supported() || !target_info.is_supported() {
            PathChangeKind::Unsupported
        } else if base_info.binary || target_info.binary {
            PathChangeKind::Binary
        } else if type_changed {
            PathChangeKind::TypeChanged
        } else if mode_changed && !content_changed {
            PathChangeKind::ModeChanged
        } else {
            PathChangeKind::ContentChanged
        };
        Some(Self {
            path,
            base,
            target,
            kind,
            content_changed,
            mode_changed,
            type_changed,
        })
    }
}

/// The repository-wide direct delta between two selected endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    base: ComparisonEndpoint,
    target: ComparisonEndpoint,
    changes: Vec<PathChange>,
}

impl Comparison {
    /// The immutable or mutable base selected for this comparison.
    #[must_use]
    pub const fn base(&self) -> &ComparisonEndpoint {
        &self.base
    }

    /// The immutable or mutable target selected for this comparison.
    #[must_use]
    pub const fn target(&self) -> &ComparisonEndpoint {
        &self.target
    }

    /// Every changed repository-relative path, in path order.
    #[must_use]
    pub fn changes(&self) -> &[PathChange] {
        &self.changes
    }

    /// The number of changed paths.
    #[must_use]
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// Whether the selected endpoints have no net path changes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub(crate) fn from_parts(
        base: ComparisonEndpoint,
        target: ComparisonEndpoint,
        changes: Vec<PathChange>,
    ) -> Self {
        Self {
            base,
            target,
            changes,
        }
    }
}

impl Default for Compare {
    fn default() -> Self {
        Self {
            context: DEFAULT_CONTEXT,
            whitespace: Whitespace::Exact,
        }
    }
}

/// Context lines `git diff` shows, the default for [`Compare`].
pub(crate) const DEFAULT_CONTEXT: usize = 3;

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
        Self::compare(old, new, Whitespace::Exact)
    }

    /// Compare `old` with `new` under `whitespace` (ADR 0060). With
    /// [`Whitespace::Ignore`] two lines that differ only in whitespace
    /// are the same line; the listing still shows the new text as it is.
    #[must_use]
    pub fn compare(old: &str, new: &str, whitespace: Whitespace) -> Self {
        let a: Vec<&str> = old.lines().collect();
        let b: Vec<&str> = new.lines().collect();
        let mut hunks = Vec::new();
        match whitespace {
            Whitespace::Exact => myers(&a, &b, 0, 0, &mut hunks),
            Whitespace::Ignore => {
                let squeeze = |lines: &[&str]| -> Vec<String> {
                    lines
                        .iter()
                        .map(|line| line.split_whitespace().collect())
                        .collect()
                };
                let (a, b) = (squeeze(&a), squeeze(&b));
                let a: Vec<&str> = a.iter().map(String::as_str).collect();
                let b: Vec<&str> = b.iter().map(String::as_str).collect();
                myers(&a, &b, 0, 0, &mut hunks);
            }
        }
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
    pub(crate) fn old_lines(&self) -> usize {
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
