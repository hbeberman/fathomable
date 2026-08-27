// @okf-doc: /decisions/0016-syntax-highlighting.md
//! Syntax highlighting for code blocks and source files (ADR 0016).
//!
//! A [`Highlighter`] wraps syntect's bundled syntax and theme sets. It maps
//! a language **hint**, a fence info string such as `rust` or a file
//! extension such as `rs`, to foreground colours per byte range of each
//! line. Only the foreground is used: the background and modifiers come
//! from the Fathomable theme so transparent terminals keep showing through.
//!
//! ```
//! use fathomable_core::highlight::Highlighter;
//!
//! let highlighter = Highlighter::new("base16-ocean.dark").unwrap();
//! let lines = highlighter.highlight("fn main() {}\n", "rs").unwrap();
//! assert_eq!(lines.len(), 1);
//! assert!(lines[0].iter().any(|run| run.range.start == 0));
//! assert!(Highlighter::plain().highlight("x", "rs").is_none());
//! ```

use std::collections::HashSet;
use std::fmt;
use std::ops::Range;
use std::sync::Mutex;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::theme::Color;

/// One coloured run inside a highlighted line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// Byte range inside the line, excluding the line terminator.
    pub range: Range<usize>,
    /// Foreground colour for the run.
    pub fg: Color,
}

/// Loaded syntaxes and one syntect theme.
pub struct Highlighter {
    inner: Option<Inner>,
    /// Hints already reported as unknown, so the log gets one line each.
    missed: Mutex<HashSet<String>>,
}

struct Inner {
    syntaxes: SyntaxSet,
    theme: Theme,
}

/// `code.syntect` named a theme syntect does not bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownTheme {
    name: String,
}

impl UnknownTheme {
    /// The name that was not found.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for UnknownTheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown syntect theme `{}`; bundled themes are {}",
            self.name,
            theme_names().join(", ")
        )
    }
}

impl std::error::Error for UnknownTheme {}

impl fmt::Debug for Highlighter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Highlighter")
            .field("enabled", &self.inner.is_some())
            .finish_non_exhaustive()
    }
}

impl Highlighter {
    /// Load the bundled syntaxes and the named bundled theme.
    ///
    /// An empty `theme` disables highlighting, as an unset `code.syntect`
    /// does (ADR 0011).
    ///
    /// # Errors
    ///
    /// Returns [`UnknownTheme`] when syntect does not bundle `theme`.
    pub fn new(theme: &str) -> Result<Self, UnknownTheme> {
        if theme.is_empty() {
            return Ok(Self::plain());
        }
        let mut themes = ThemeSet::load_defaults();
        let Some(theme) = themes.themes.remove(theme) else {
            return Err(UnknownTheme {
                name: theme.to_owned(),
            });
        };
        Ok(Self {
            inner: Some(Inner {
                syntaxes: SyntaxSet::load_defaults_newlines(),
                theme,
            }),
            missed: Mutex::new(HashSet::new()),
        })
    }

    /// A highlighter that never colours anything.
    #[must_use]
    pub fn plain() -> Self {
        Self {
            inner: None,
            missed: Mutex::new(HashSet::new()),
        }
    }

    /// Whether a theme is loaded.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Whether `hint` names a language this highlighter knows.
    #[must_use]
    pub fn knows(&self, hint: &str) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|inner| inner.syntaxes.find_syntax_by_token(hint).is_some())
    }

    /// Colour `text` as the language `hint` names, one entry per line of
    /// `text` (as `str::lines` counts them). `None` when highlighting is
    /// off or the hint is unknown, in which case the text renders plain.
    #[must_use]
    pub fn highlight(&self, text: &str, hint: &str) -> Option<Vec<Vec<Run>>> {
        let inner = self.inner.as_ref()?;
        let Some(syntax) = inner.syntaxes.find_syntax_by_token(hint) else {
            self.miss(hint);
            return None;
        };
        let mut lines = HighlightLines::new(syntax, &inner.theme);
        let mut out = Vec::new();
        for line in LinesWithEndings::from(text) {
            let content = line.trim_end_matches(['\n', '\r']);
            let ranges = match lines.highlight_line(line, &inner.syntaxes) {
                Ok(ranges) => ranges,
                Err(error) => {
                    tracing::debug!(hint, %error, "highlighting failed; rendering plain");
                    return None;
                }
            };
            let mut runs = Vec::new();
            let mut start = 0;
            for (style, piece) in ranges {
                let end = (start + piece.len()).min(content.len());
                if end > start {
                    let fg = style.foreground;
                    runs.push(Run {
                        range: start..end,
                        fg: Color::Rgb(fg.r, fg.g, fg.b),
                    });
                }
                start += piece.len();
            }
            out.push(runs);
        }
        Some(out)
    }

    fn miss(&self, hint: &str) {
        if let Ok(mut missed) = self.missed.lock()
            && missed.insert(hint.to_owned())
        {
            tracing::debug!(hint, "no syntax for language hint; rendering plain");
        }
    }
}

/// The syntect theme names `code.syntect` may use, sorted.
#[must_use]
pub fn theme_names() -> Vec<String> {
    ThemeSet::load_defaults().themes.into_keys().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn unknown_theme_is_an_error_naming_the_bundle() {
        let error = Highlighter::new("nope")
            .err()
            .map(|e| (e.name().to_owned(), e.to_string()));
        let (name, message) = error.unwrap_or_default();
        assert_eq!(name, "nope");
        assert!(message.contains("base16-ocean.dark"), "{message}");
    }

    #[test]
    fn empty_theme_disables_highlighting() -> TestResult {
        let highlighter = Highlighter::new("")?;
        assert!(!highlighter.is_enabled());
        assert!(highlighter.highlight("fn x() {}", "rs").is_none());
        Ok(())
    }

    #[test]
    fn unknown_hint_renders_plain() -> TestResult {
        let highlighter = Highlighter::new("base16-ocean.dark")?;
        assert!(!highlighter.knows("no-such-language"));
        assert!(highlighter.highlight("x", "no-such-language").is_none());
        Ok(())
    }

    #[test]
    fn runs_cover_each_line_without_the_newline() -> TestResult {
        let highlighter = Highlighter::new("base16-ocean.dark")?;
        let text = "fn main() {\n    let x = 1;\n}\n";
        let lines = highlighter
            .highlight(text, "rust")
            .ok_or("rust is bundled")?;
        assert_eq!(lines.len(), 3);
        for (line, runs) in text.lines().zip(&lines) {
            let end = runs.last().map_or(0, |run| run.range.end);
            assert_eq!(end, line.len(), "line {line:?}");
        }
        let keyword = lines[0].first().ok_or("first run")?;
        let last = lines[0].last().ok_or("last run")?;
        assert_eq!(keyword.range, 0..2);
        assert_ne!(keyword.fg, last.fg);
        Ok(())
    }

    #[test]
    fn theme_names_are_sorted_and_bundled() {
        let names = theme_names();
        assert!(names.contains(&"base16-ocean.dark".to_owned()));
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
