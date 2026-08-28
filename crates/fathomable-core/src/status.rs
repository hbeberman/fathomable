// @okf-doc: /decisions/0017-git-status-navigation.md
//! The dirty set: every uncommitted path in the workspace, staged and
//! unstaged told apart (ADR 0017).
//!
//! [`Status`] is what [`Workspace::status`](crate::workspace::Workspace::status)
//! returns: one [`Entry`] per dirty path, sorted by path, with the line
//! counts against `HEAD` the sidebar shows. It is plain data; stepping
//! through it in path order is what `]G` and `[G` do, and `]g` crosses
//! into the next entry when a file's hunks run out.
//!
//! # Examples
//!
//! ```
//! use std::path::{Path, PathBuf};
//!
//! use fathomable_core::status::{Entry, State, Status};
//!
//! let status = Status::from_entries(vec![
//!     Entry::new(PathBuf::from("src/b.rs"), State::Modified, 2, 1),
//!     Entry::new(PathBuf::from("src/a.rs"), State::Untracked, 9, 0),
//! ]);
//! assert_eq!(status.len(), 2);
//! assert_eq!(status.entries()[0].path(), Path::new("src/a.rs"));
//! let src = status.summary_under(Path::new("src")).unwrap();
//! assert_eq!((src.state, src.added, src.removed), (State::Untracked, 11, 1));
//! assert_eq!(status.after(Some(Path::new("src/b.rs"))).map(Entry::path), Some(Path::new("src/a.rs")));
//! ```

use std::fmt;
use std::path::{Path, PathBuf};

/// How a path differs from `HEAD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum State {
    /// Tracked and changed.
    Modified,
    /// Deleted from the working tree or the index.
    Deleted,
    /// Added to the index; not in `HEAD`.
    Added,
    /// On disk, not ignored, not in the index.
    Untracked,
}

impl State {
    /// The sidebar letter.
    #[must_use]
    pub const fn letter(self) -> char {
        match self {
            Self::Modified => 'M',
            Self::Deleted => 'D',
            Self::Added => 'A',
            Self::Untracked => '?',
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Added => "added",
            Self::Untracked => "untracked",
        })
    }
}

/// One dirty path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    path: PathBuf,
    state: State,
    staged: bool,
    binary: bool,
    added: usize,
    removed: usize,
}

impl Entry {
    /// An unstaged dirty `path` in `state`; `added` and `removed` are
    /// line counts of the working tree against `HEAD`.
    #[must_use]
    pub fn new(path: PathBuf, state: State, added: usize, removed: usize) -> Self {
        Self {
            path,
            state,
            staged: false,
            binary: false,
            added,
            removed,
        }
    }

    /// Mark the entry as binary (ADR 0026): it has no line counts, and
    /// the sidebar tags it instead.
    #[must_use]
    pub fn binary(mut self) -> Self {
        self.binary = true;
        self
    }

    /// Mark the entry as staged: the index differs from `HEAD` for it.
    #[must_use]
    pub fn staged(mut self) -> Self {
        self.staged = true;
        self
    }

    /// Root-relative path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How the path differs; for a path that is both staged and further
    /// edited, the working tree's state.
    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    /// Whether the index differs from `HEAD` for this path.
    #[must_use]
    pub fn is_staged(&self) -> bool {
        self.staged
    }

    /// Whether the file is binary by git's rule (ADR 0026).
    #[must_use]
    pub fn is_binary(&self) -> bool {
        self.binary
    }

    /// Lines added against `HEAD`.
    #[must_use]
    pub fn added(&self) -> usize {
        self.added
    }

    /// Lines removed against `HEAD`.
    #[must_use]
    pub fn removed(&self) -> usize {
        self.removed
    }
}

/// What a collapsed directory shows for the dirty paths beneath it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// The most advanced state beneath (`?` > `A` > `D` > `M`).
    pub state: State,
    /// Whether every dirty path beneath is staged.
    pub staged: bool,
    /// Summed additions.
    pub added: usize,
    /// Summed removals.
    pub removed: usize,
    /// Whether every dirty path beneath is binary (ADR 0026).
    pub binary: bool,
}

/// The dirty set, sorted by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    entries: Vec<Entry>,
}

impl Status {
    /// Build from entries in any order; duplicates keep the last.
    #[must_use]
    pub fn from_entries(mut entries: Vec<Entry>) -> Self {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries.dedup_by(|later, earlier| {
            if later.path == earlier.path {
                std::mem::swap(later, earlier);
                true
            } else {
                false
            }
        });
        Self { entries }
    }

    /// Every dirty path, in path order.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many paths are dirty.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is uncommitted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry for `path`, when it is dirty.
    #[must_use]
    pub fn get(&self, path: &Path) -> Option<&Entry> {
        self.entries
            .binary_search_by(|entry| entry.path.as_path().cmp(path))
            .ok()
            .map(|index| &self.entries[index])
    }

    /// Whether `path` is dirty.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.get(path).is_some()
    }

    /// The dirty paths under directory `dir`, folded for a collapsed row;
    /// `None` when none are.
    #[must_use]
    pub fn summary_under(&self, dir: &Path) -> Option<Summary> {
        let mut summary: Option<Summary> = None;
        for entry in self.entries.iter().filter(|e| e.path.starts_with(dir)) {
            summary = Some(match summary {
                None => Summary {
                    state: entry.state,
                    staged: entry.staged,
                    added: entry.added,
                    removed: entry.removed,
                    binary: entry.binary,
                },
                Some(acc) => Summary {
                    state: acc.state.max(entry.state),
                    staged: acc.staged && entry.staged,
                    added: acc.added + entry.added,
                    removed: acc.removed + entry.removed,
                    binary: acc.binary && entry.binary,
                },
            });
        }
        summary
    }

    /// The entry after `current` in path order, wrapping to the first;
    /// the first when `current` is `None` or not dirty. `None` when the set
    /// is empty.
    #[must_use]
    pub fn after(&self, current: Option<&Path>) -> Option<&Entry> {
        if self.entries.is_empty() {
            return None;
        }
        let index = match current.map(|path| {
            self.entries
                .binary_search_by(|entry| entry.path.as_path().cmp(path))
        }) {
            Some(Ok(found)) => (found + 1) % self.entries.len(),
            Some(Err(insert)) => insert % self.entries.len(),
            None => 0,
        };
        self.entries.get(index)
    }

    /// The entry before `current` in path order, wrapping to the last;
    /// the last when `current` is `None` or not dirty.
    #[must_use]
    pub fn before(&self, current: Option<&Path>) -> Option<&Entry> {
        if self.entries.is_empty() {
            return None;
        }
        let last = self.entries.len() - 1;
        let index = match current.map(|path| {
            self.entries
                .binary_search_by(|entry| entry.path.as_path().cmp(path))
        }) {
            Some(Ok(0) | Err(0)) | None => last,
            Some(Ok(found)) => found - 1,
            Some(Err(insert)) => insert - 1,
        };
        self.entries.get(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, state: State, staged: bool) -> Entry {
        let entry = Entry::new(PathBuf::from(path), state, 1, 1);
        if staged { entry.staged() } else { entry }
    }

    #[test]
    fn entries_sort_and_dedup_by_path() {
        let status = Status::from_entries(vec![
            entry("b", State::Modified, false),
            entry("a", State::Modified, false),
            entry("b", State::Added, true),
        ]);
        assert_eq!(status.len(), 2);
        assert_eq!(status.entries()[1].state(), State::Added);
        assert!(status.entries()[1].is_staged());
        assert!(status.contains(Path::new("a")));
        assert!(!status.contains(Path::new("c")));
    }

    #[test]
    fn stepping_wraps_in_path_order() {
        let status = Status::from_entries(vec![
            entry("a", State::Modified, false),
            entry("c", State::Modified, false),
        ]);
        let path = |e: Option<&Entry>| e.map(|e| e.path().display().to_string());
        assert_eq!(path(status.after(None)).as_deref(), Some("a"));
        assert_eq!(
            path(status.after(Some(Path::new("a")))).as_deref(),
            Some("c")
        );
        assert_eq!(
            path(status.after(Some(Path::new("c")))).as_deref(),
            Some("a")
        );
        assert_eq!(
            path(status.after(Some(Path::new("b")))).as_deref(),
            Some("c")
        );
        assert_eq!(path(status.before(None)).as_deref(), Some("c"));
        assert_eq!(
            path(status.before(Some(Path::new("a")))).as_deref(),
            Some("c")
        );
        assert_eq!(
            path(status.before(Some(Path::new("b")))).as_deref(),
            Some("a")
        );
        assert_eq!(
            path(status.before(Some(Path::new("c")))).as_deref(),
            Some("a")
        );
        assert!(Status::default().after(None).is_none());
        assert!(Status::default().before(None).is_none());
    }

    #[test]
    fn summary_folds_state_staging_and_counts() {
        let status = Status::from_entries(vec![
            Entry::new(PathBuf::from("d/a"), State::Modified, 1, 2).staged(),
            Entry::new(PathBuf::from("d/b"), State::Untracked, 3, 0),
            Entry::new(PathBuf::from("e"), State::Deleted, 0, 5).staged(),
        ]);
        assert_eq!(
            status.summary_under(Path::new("d")),
            Some(Summary {
                state: State::Untracked,
                staged: false,
                added: 4,
                removed: 2,
                binary: false,
            })
        );
        assert_eq!(
            status.summary_under(Path::new("e")).map(|s| s.staged),
            Some(true)
        );
        assert!(status.summary_under(Path::new("f")).is_none());
    }
}
