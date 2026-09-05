// @okf-doc: /decisions/0015-follow-mode.md
//! Follow mode: which changes the viewer reacts to and the queue of
//! changed files a jump key walks.
//!
//! [`Ignore`] is the `follow.ignore` glob list. [`Queue`] keeps one
//! [`Change`] per file, newest first; a later change to a queued file
//! moves it to the front. Stepping newest-first and oldest-first is what
//! `]f` and `[f` do.
//!
//! # Examples
//!
//! ```
//! use std::path::PathBuf;
//!
//! use fathomable_core::follow::{Change, Queue, Target};
//!
//! let mut queue = Queue::default();
//! queue.push(Change::new(PathBuf::from("a.rs"), Target::Line(3)));
//! queue.push(Change::new(PathBuf::from("b.rs"), Target::Line(1)));
//! queue.push(Change::new(PathBuf::from("a.rs"), Target::Line(9)));
//!
//! assert_eq!(queue.len(), 2);
//! assert_eq!(queue.newest().map(|c| c.path.as_path()), Some("a.rs".as_ref()));
//! assert_eq!(queue.newest().map(|c| c.target), Some(Target::Line(9)));
//! ```

use std::fmt;
use std::path::{Path, PathBuf};

use gix::bstr::BStr;
use gix::glob::pattern::Case;
use gix::glob::wildmatch;

/// The `follow.ignore` globs, matched against root-relative paths.
///
/// Patterns use gitignore syntax: `*` does not cross `/`, `**` does, and a
/// trailing `/` means a directory.
#[derive(Debug, Clone, Default)]
pub struct Ignore {
    patterns: Vec<gix::glob::Pattern>,
}

impl Ignore {
    /// Compile `globs`; an empty or negated pattern is reported by index.
    ///
    /// # Errors
    ///
    /// Returns the offending glob when it cannot be parsed.
    pub fn new(globs: &[String]) -> Result<Self, UnknownGlob> {
        let patterns = globs
            .iter()
            .map(|glob| {
                gix::glob::Pattern::from_bytes_without_negation(glob.as_bytes())
                    .filter(|_| !glob.starts_with('!'))
                    .ok_or_else(|| UnknownGlob(glob.clone()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { patterns })
    }

    /// Whether the root-relative `path` (a file) matches any glob, or sits
    /// under a directory that does.
    #[must_use]
    pub fn is_ignored(&self, path: &Path) -> bool {
        let text = path.to_string_lossy();
        let text = text.trim_start_matches("./");
        let mut end = text.len();
        loop {
            let candidate = &text[..end];
            let is_dir = end != text.len();
            let basename = candidate.rfind('/').map(|p| p + 1);
            if self.patterns.iter().any(|pattern| {
                pattern.matches_repo_relative_path(
                    BStr::new(candidate),
                    basename,
                    Some(is_dir),
                    Case::Sensitive,
                    wildmatch::Mode::NO_MATCH_SLASH_LITERAL,
                )
            }) {
                return true;
            }
            match candidate.rfind('/') {
                Some(slash) => end = slash,
                None => return false,
            }
        }
    }
}

/// A `follow.ignore` glob that cannot be compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownGlob(pub String);

impl fmt::Display for UnknownGlob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid follow ignore glob {:?}", self.0)
    }
}

impl std::error::Error for UnknownGlob {}

/// Where a jump lands in a changed file (1-based lines).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A single line: the first hunk against the last-seen base, or line 1.
    Line(usize),
    /// An inclusive range an agent asked to `open`.
    Range(usize, usize),
}

impl Target {
    /// The row a jump scrolls to.
    #[must_use]
    pub const fn line(self) -> usize {
        match self {
            Self::Line(line) | Self::Range(line, _) => line,
        }
    }
}

/// One changed file and where to land in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Root-relative path.
    pub path: PathBuf,
    /// Where the jump lands.
    pub target: Target,
}

impl Change {
    /// A change to `path` landing at `target`.
    #[must_use]
    pub const fn new(path: PathBuf, target: Target) -> Self {
        Self { path, target }
    }
}

/// Changed files, newest first, at most one entry per path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queue {
    changes: Vec<Change>,
}

impl Queue {
    /// An empty queue.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            changes: Vec::new(),
        }
    }

    /// Record a change, moving an already queued path to the front with the
    /// new target.
    pub fn push(&mut self, change: Change) {
        self.changes.retain(|queued| queued.path != change.path);
        self.changes.insert(0, change);
    }

    /// Drop the entry for `path`, if any; true when one was removed.
    pub fn remove(&mut self, path: &Path) -> bool {
        let before = self.changes.len();
        self.changes.retain(|queued| queued.path != path);
        before != self.changes.len()
    }

    /// Drop every entry (`Space j c`).
    pub fn clear(&mut self) {
        self.changes.clear();
    }

    /// The most recent change.
    #[must_use]
    pub fn newest(&self) -> Option<&Change> {
        self.changes.first()
    }

    /// Whether `path` is queued.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.changes.iter().any(|queued| queued.path == path)
    }

    /// The change to visit after `current` going towards older entries
    /// (`]f`): the next older one when `current` is queued, else the newest.
    /// Wraps to the newest after the oldest.
    #[must_use]
    pub fn after(&self, current: Option<&Path>) -> Option<&Change> {
        let index = self
            .position(current)
            .map_or(0, |i| (i + 1) % self.changes.len());
        self.changes.get(index)
    }

    /// The change to visit before `current` going towards newer entries
    /// (`[f`): the next newer one when `current` is queued, else the oldest.
    /// Wraps to the oldest after the newest.
    #[must_use]
    pub fn before(&self, current: Option<&Path>) -> Option<&Change> {
        let len = self.changes.len();
        let index = self
            .position(current)
            .map_or(len.checked_sub(1), |i| Some((i + len - 1) % len))?;
        self.changes.get(index)
    }

    /// Queued changes, newest first.
    pub fn iter(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter()
    }

    /// Number of queued files.
    #[must_use]
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// Whether nothing is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    fn position(&self, path: Option<&Path>) -> Option<usize> {
        let path = path?;
        self.changes.iter().position(|queued| queued.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(name: &str, line: usize) -> Change {
        Change::new(PathBuf::from(name), Target::Line(line))
    }

    fn names(queue: &Queue) -> Vec<&str> {
        queue
            .iter()
            .map(|c| c.path.to_str().unwrap_or_default())
            .collect()
    }

    #[test]
    fn push_keeps_one_entry_per_path_newest_first() {
        let mut queue = Queue::default();
        queue.push(change("a", 1));
        queue.push(change("b", 1));
        queue.push(change("c", 1));
        queue.push(change("a", 7));
        assert_eq!(names(&queue), ["a", "c", "b"]);
        assert_eq!(queue.newest().map(|c| c.target), Some(Target::Line(7)));
    }

    #[test]
    fn remove_and_clear() {
        let mut queue = Queue::default();
        queue.push(change("a", 1));
        queue.push(change("b", 1));
        assert!(queue.remove(Path::new("a")));
        assert!(!queue.remove(Path::new("a")));
        assert!(queue.contains(Path::new("b")));
        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.newest(), None);
    }

    #[test]
    fn after_walks_towards_older_and_wraps() {
        let mut queue = Queue::default();
        queue.push(change("old", 1));
        queue.push(change("mid", 1));
        queue.push(change("new", 1));
        let path = |c: Option<&Change>| c.map(|c| c.path.clone());
        assert_eq!(path(queue.after(None)), Some(PathBuf::from("new")));
        assert_eq!(
            path(queue.after(Some(Path::new("new")))),
            Some(PathBuf::from("mid"))
        );
        assert_eq!(
            path(queue.after(Some(Path::new("old")))),
            Some(PathBuf::from("new"))
        );
        assert_eq!(
            path(queue.after(Some(Path::new("not queued")))),
            Some(PathBuf::from("new"))
        );
    }

    #[test]
    fn before_walks_towards_newer_and_wraps() {
        let mut queue = Queue::default();
        queue.push(change("old", 1));
        queue.push(change("mid", 1));
        queue.push(change("new", 1));
        let path = |c: Option<&Change>| c.map(|c| c.path.clone());
        assert_eq!(path(queue.before(None)), Some(PathBuf::from("old")));
        assert_eq!(
            path(queue.before(Some(Path::new("old")))),
            Some(PathBuf::from("mid"))
        );
        assert_eq!(
            path(queue.before(Some(Path::new("new")))),
            Some(PathBuf::from("old"))
        );
    }

    #[test]
    fn empty_queue_has_no_neighbours() {
        let queue = Queue::default();
        assert_eq!(queue.after(None), None);
        assert_eq!(queue.before(None), None);
        assert_eq!(queue.after(Some(Path::new("a"))), None);
    }

    #[test]
    fn ignore_globs_follow_gitignore_rules() {
        let ignore = Ignore::new(&[
            "target/**".to_owned(),
            "*.lock".to_owned(),
            "docs/".to_owned(),
            "build".to_owned(),
        ])
        .map_err(|e| e.to_string());
        let ignore = ignore.unwrap_or_default();
        assert!(ignore.is_ignored(Path::new("target/debug/app")));
        assert!(ignore.is_ignored(Path::new("Cargo.lock")));
        assert!(ignore.is_ignored(Path::new("sub/Cargo.lock")));
        assert!(ignore.is_ignored(Path::new("docs/guide.md")));
        assert!(ignore.is_ignored(Path::new("a/build/out.o")));
        assert!(!ignore.is_ignored(Path::new("src/main.rs")));
        assert!(!ignore.is_ignored(Path::new("targets/x")));
        assert!(!ignore.is_ignored(Path::new("docs")));
        assert_eq!(
            Ignore::new(&["!x".to_owned()]).err(),
            Some(UnknownGlob("!x".to_owned()))
        );
        assert!(!Ignore::default().is_ignored(Path::new("anything")));
    }

    #[test]
    fn target_line() {
        assert_eq!(Target::Line(4).line(), 4);
        assert_eq!(Target::Range(2, 9).line(), 2);
    }
}
