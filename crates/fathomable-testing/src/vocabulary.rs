//! Test-only checks for identifiers in agent-facing prose.

use fathomable_core::vocabulary::{
    ALL, ANSWERED, PENDING, STATUS_ALL, STATUS_OPEN, WHEN_MESSAGE, WHEN_RESOLVED, WORKTREE,
};

/// Whether `ident` is a tool name, a parameter name, a `when` or
/// `status` value, or the `pending`, `answered`, or `worktree` field.
#[must_use]
pub fn is_known(ident: &str) -> bool {
    [
        WHEN_MESSAGE,
        WHEN_RESOLVED,
        STATUS_OPEN,
        STATUS_ALL,
        PENDING,
        ANSWERED,
        WORKTREE,
    ]
    .contains(&ident)
        || ALL
            .iter()
            .any(|tool| tool.name == ident || tool.params.contains(&ident))
}

/// The backticked identifiers of `text`, in order, repeats included.
///
/// A span is an identifier when it is one word of letters, digits,
/// underscores, dots, or dashes; a span with spaces or other punctuation
/// (a command line, a call shape) is skipped.
pub fn idents(text: &str) -> impl Iterator<Item = &str> {
    text.split('`').skip(1).step_by(2).filter(|span| {
        !span.is_empty()
            && span
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    })
}

#[cfg(test)]
mod tests {
    use super::{idents, is_known};

    #[test]
    fn idents_picks_single_words_only() {
        let text = "call `follow` with `type`, not `fathomable --mcp` or ``; `when` is `message`";
        let found: Vec<_> = idents(text).collect();
        assert_eq!(found, ["follow", "type", "when", "message"]);
        assert!(found.iter().all(|ident| is_known(ident)));
        assert!(!is_known("fathomable"));
    }
}
