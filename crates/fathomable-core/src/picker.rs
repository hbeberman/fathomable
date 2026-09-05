// @okf-doc: /decisions/0012-workspace-mode.md
//! Fuzzy matching over a list of candidate strings for the file picker.
//!
//! [`Picker`] wraps `nucleo-matcher` (ADR 0012) with the Helix pattern
//! syntax: words are matched fuzzily, `^`/`$` anchor, `!` negates, and a
//! leading `'` asks for a substring. Case is smart: lowercase queries are
//! case-insensitive. Matching is synchronous; the caller decides how many of
//! the ranked results to show.
//!
//! # Examples
//!
//! ```
//! use fathomable_core::picker::Picker;
//!
//! let mut picker = Picker::new(vec!["src/main.rs".to_owned(), "README.md".to_owned()]);
//! let ranked = picker.query("srm");
//! assert_eq!(picker.items()[ranked[0].index()], "src/main.rs");
//! ```

use std::fmt;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// One ranked result of a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    index: usize,
    score: u32,
    positions: Vec<u32>,
}

impl Match {
    /// Index into [`Picker::items`].
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Character positions in the item that matched the query, ascending.
    #[must_use]
    pub fn positions(&self) -> &[u32] {
        &self.positions
    }
}

/// A list of candidates and a matcher to rank them.
pub struct Picker {
    items: Vec<String>,
    matcher: Matcher,
}

impl fmt::Debug for Picker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Picker")
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}

impl Picker {
    /// Rank over `items`, which are usually root-relative paths.
    #[must_use]
    pub fn new(items: Vec<String>) -> Self {
        Self {
            items,
            matcher: Matcher::new(Config::DEFAULT.match_paths()),
        }
    }

    /// The candidates, in the order they were given.
    #[must_use]
    pub fn items(&self) -> &[String] {
        &self.items
    }

    /// Rank the items against `query`, best first. An empty query lists
    /// every item in its original order with no highlighted positions.
    pub fn query(&mut self, query: &str) -> Vec<Match> {
        if query.trim().is_empty() {
            return (0..self.items.len())
                .map(|index| Match {
                    index,
                    score: 0,
                    positions: Vec::new(),
                })
                .collect();
        }
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut positions = Vec::new();
        let mut matches: Vec<Match> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let haystack = Utf32Str::new(item, &mut buf);
                positions.clear();
                let score = pattern.indices(haystack, &mut self.matcher, &mut positions)?;
                positions.sort_unstable();
                positions.dedup();
                Some(Match {
                    index,
                    score,
                    positions: positions.clone(),
                })
            })
            .collect();
        // Stable so equal scores keep listing order (shallower paths first
        // when the index came from a depth-first walk).
        matches.sort_by_key(|m| std::cmp::Reverse(m.score));
        matches
    }
}

#[cfg(test)]
mod tests {
    use super::Picker;

    fn picker() -> Picker {
        Picker::new(
            [
                "docs/index.md",
                "src/main.rs",
                "src/viewer/mod.rs",
                "README.md",
            ]
            .map(str::to_owned)
            .to_vec(),
        )
    }

    #[test]
    fn empty_query_lists_everything_in_order() {
        let mut p = picker();
        let all = p.query("  ");
        assert_eq!(
            all.iter().map(super::Match::index).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert!(all.iter().all(|m| m.positions().is_empty()));
    }

    #[test]
    fn fuzzy_query_ranks_and_reports_positions() {
        let mut p = picker();
        let ranked = p.query("vmod");
        assert_eq!(p.items()[ranked[0].index()], "src/viewer/mod.rs");
        let positions = ranked[0].positions();
        assert!(
            positions.contains(&4),
            "v of viewer at index 4: {positions:?}"
        );
        assert!(positions.windows(2).all(|w| w[0] < w[1]));
        assert!(p.query("zzzz").is_empty());
    }

    #[test]
    fn helix_syntax_and_smart_case() {
        let mut p = picker();
        let anchored = p.query("^src");
        assert_eq!(anchored.len(), 2);
        let negated = p.query("md !readme");
        assert!(!negated.is_empty());
        assert!(negated.iter().all(|m| p.items()[m.index()] != "README.md"));
        assert_eq!(p.query("readme").len(), 1, "lowercase ignores case");
        assert!(p.query("Readme").is_empty(), "uppercase is exact");
    }
}
