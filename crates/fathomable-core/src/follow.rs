// @okf-doc: /decisions/0015-follow-mode.md
//! Live-change ignore rules.
//!
//! [`Ignore`] is the `watch.ignore` glob list.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::follow::Ignore;
//!
//! let ignore = Ignore::new(&["target/**".to_owned()]).unwrap();
//! assert!(ignore.is_ignored("target/debug/app".as_ref()));
//! ```

use std::fmt;
use std::path::Path;

use gix::bstr::BStr;
use gix::glob::pattern::Case;
use gix::glob::wildmatch;

/// The `watch.ignore` globs, matched against root-relative paths.
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

    /// Whether `path` is an ignored directory whose whole subtree can be
    /// skipped.
    ///
    /// A pattern such as `target/**` or `docs/` matches a child rather than
    /// the directory name itself. Testing one ordinary child distinguishes
    /// those subtree patterns from file-only patterns such as `*.lock`.
    #[must_use]
    pub fn is_tree_ignored(&self, path: &Path) -> bool {
        self.is_ignored(path) || self.is_ignored(&path.join(".fathomable-watch-probe"))
    }
}

/// A `watch.ignore` glob that cannot be compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownGlob(pub String);

impl fmt::Display for UnknownGlob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid follow ignore glob {:?}", self.0)
    }
}

impl std::error::Error for UnknownGlob {}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(ignore.is_tree_ignored(Path::new("target")));
        assert!(ignore.is_tree_ignored(Path::new("docs")));
        assert!(!ignore.is_tree_ignored(Path::new("src")));
        assert!(!ignore.is_ignored(Path::new("src/main.rs")));
        assert!(!ignore.is_ignored(Path::new("targets/x")));
        assert!(!ignore.is_ignored(Path::new("docs")));
        assert_eq!(
            Ignore::new(&["!x".to_owned()]).err(),
            Some(UnknownGlob("!x".to_owned()))
        );
        assert!(!Ignore::default().is_ignored(Path::new("anything")));
    }
}
