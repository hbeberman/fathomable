// @okf-doc: /decisions/0052-goto-file.md
//! File references in text: what `gf` opens (ADR 0052).
//!
//! A reference is a Markdown link destination or a bare word such as
//! `crates/foo/src/lib.rs:42`. [`parse`] splits it into a path and an
//! optional 1-based line, reading the line from a trailing `:LINE` or
//! `:LINE:COL` (compiler style) or from a `#L…` fragment (code host
//! style). [`word_at`] picks the reference out of a rendered line of
//! text around a byte offset, trimming the quotes, brackets, and sentence
//! punctuation prose wraps it in. Neither touches the file system: which
//! directory a relative path is read against is the caller's decision.
//!
//! # Examples
//!
//! ```
//! use std::path::Path;
//!
//! use fathomable_core::link;
//!
//! let word = link::word_at("see `src/lib.rs:42`.", 8).unwrap();
//! assert_eq!(word, "src/lib.rs:42");
//! let target = link::parse(word).unwrap();
//! assert_eq!(target.path(), Path::new("src/lib.rs"));
//! assert_eq!(target.line(), Some(42));
//! assert!(link::parse("https://example.com/a.md").is_none());
//! ```

use std::path::{Path, PathBuf};

/// A file a reference names, with the line it points at when it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    path: PathBuf,
    line: Option<usize>,
}

impl Target {
    /// The path as written, relative or absolute; not resolved.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The 1-based line the reference points at, if it names one.
    #[must_use]
    pub fn line(&self) -> Option<usize> {
        self.line
    }
}

/// Split a reference into its path and line, or `None` when it is not a
/// file: empty, only a fragment, or carrying a scheme (`://`, `mailto:`).
///
/// A trailing `:LINE` or `:LINE:COL` gives the line and drops the column;
/// a `#LLINE`, `#LLINE-LEND`, or `#LLINEC…` fragment gives the line and
/// any other fragment is dropped. `%XX` escapes are decoded.
#[must_use]
pub fn parse(reference: &str) -> Option<Target> {
    if reference.contains("://") || reference.starts_with("mailto:") {
        return None;
    }
    let (rest, fragment) = match reference.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (reference, None),
    };
    let (rest, suffix_line) = split_line_suffix(rest);
    let line = suffix_line.or_else(|| fragment.and_then(fragment_line));
    let path = decode(rest);
    if path.is_empty() {
        return None;
    }
    Some(Target {
        path: PathBuf::from(path),
        line,
    })
}

/// The reference the byte at `offset` in `text` is part of: the longest
/// run of path characters around it with wrapping quotes, brackets, and
/// trailing sentence punctuation trimmed. `None` off the text, on a
/// space, or when nothing but punctuation is left.
#[must_use]
pub fn word_at(text: &str, offset: usize) -> Option<&str> {
    if offset >= text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    if !text[offset..].starts_with(is_path_char) {
        return None;
    }
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_path_char(c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|&(_, c)| !is_path_char(c))
        .map_or(text.len(), |(i, _)| offset + i);
    let run = &text[start..end];
    // Raw Markdown, as the source view shows it: the destination is the
    // reference, whatever the cursor is on.
    let run = run.rsplit_once("](").map_or(run, |(_, dest)| dest);
    let word = run
        .trim_start_matches(|c| OPENERS.contains(c))
        .trim_end_matches(|c| CLOSERS.contains(c));
    (!word.is_empty()).then_some(word)
}

/// Characters a reference is made of, and the quoting prose puts around
/// one. Quotes and brackets are in so a word like `(docs/a.md)` is found
/// whole, then trimmed.
fn is_path_char(c: char) -> bool {
    c.is_alphanumeric() || "/._-~:#@+%=()[]<>'\"`".contains(c)
}

const OPENERS: &str = "([<'\"`";
const CLOSERS: &str = ")]>'\"`.,;:!?";

/// Strip a trailing `:LINE` or `:LINE:COL` and return the line.
fn split_line_suffix(text: &str) -> (&str, Option<usize>) {
    let Some((head, last)) = text.rsplit_once(':') else {
        return (text, None);
    };
    let Some(last) = number(last) else {
        return (text, None);
    };
    // `:LINE:COL`: the earlier number is the line.
    if let Some((path, line)) = head.rsplit_once(':')
        && let Some(line) = number(line)
        && !path.is_empty()
    {
        return (path, Some(line));
    }
    if head.is_empty() {
        return (text, None);
    }
    (head, Some(last))
}

/// The line a code-host fragment names: `L12`, `L12-L20`, `L12C3`.
fn fragment_line(fragment: &str) -> Option<usize> {
    let digits = fragment.strip_prefix('L')?;
    let end = digits
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(digits.len());
    number(&digits[..end])
}

fn number(text: &str) -> Option<usize> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
        .filter(|&n| n > 0)
}

/// Decode `%XX` escapes; a malformed escape stays as written.
fn decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_owned();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| text.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        if let Some(byte) = escaped {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Target, parse, word_at};

    fn target(path: &str, line: Option<usize>) -> Target {
        Target {
            path: Path::new(path).to_path_buf(),
            line,
        }
    }

    #[test]
    fn compiler_style_suffix_gives_the_line_and_drops_the_column() {
        assert_eq!(parse("src/lib.rs:42"), Some(target("src/lib.rs", Some(42))));
        assert_eq!(
            parse("src/lib.rs:42:7"),
            Some(target("src/lib.rs", Some(42)))
        );
        assert_eq!(parse("src/lib.rs"), Some(target("src/lib.rs", None)));
        assert_eq!(parse("../a.md"), Some(target("../a.md", None)));
    }

    #[test]
    fn code_host_fragment_gives_the_line_and_other_fragments_drop() {
        assert_eq!(parse("a.rs#L12"), Some(target("a.rs", Some(12))));
        assert_eq!(parse("a.rs#L12-L20"), Some(target("a.rs", Some(12))));
        assert_eq!(parse("a.rs#L12C3"), Some(target("a.rs", Some(12))));
        assert_eq!(parse("guide.md#setup"), Some(target("guide.md", None)));
        assert_eq!(parse("#setup"), None);
    }

    #[test]
    fn schemes_and_empty_references_are_not_files() {
        assert_eq!(parse("https://example.com/a.md"), None);
        assert_eq!(parse("file:///tmp/a.md"), None);
        assert_eq!(parse("mailto:someone@example.com"), None);
        assert_eq!(parse(""), None);
        assert_eq!(
            parse(":12"),
            Some(target(":12", None)),
            "no path before the colon"
        );
    }

    #[test]
    fn line_zero_and_non_numbers_are_not_lines() {
        assert_eq!(parse("a.rs:0"), Some(target("a.rs:0", None)));
        assert_eq!(parse("C:pictures"), Some(target("C:pictures", None)));
    }

    #[test]
    fn percent_escapes_decode() {
        assert_eq!(parse("my%20file.md"), Some(target("my file.md", None)));
        assert_eq!(parse("odd%2"), Some(target("odd%2", None)));
    }

    #[test]
    fn word_is_the_run_of_path_characters_trimmed_of_wrapping() {
        let text = "see (docs/guide.md:12), then `src/a.rs`.";
        assert_eq!(word_at(text, 6), Some("docs/guide.md:12"));
        assert_eq!(
            word_at(text, 4),
            Some("docs/guide.md:12"),
            "from the bracket"
        );
        assert_eq!(word_at(text, 30), Some("src/a.rs"));
        assert_eq!(word_at(text, 3), None, "a space");
        assert_eq!(word_at(text, 0), Some("see"));
        assert_eq!(word_at(text, text.len()), None, "off the end");
        assert_eq!(word_at("[x](a.md)", 1), Some("a.md"), "raw Markdown");
        assert_eq!(word_at("(\")", 0), None, "only punctuation");
    }
}
