// @okf-doc: /decisions/0075-the-header-names-its-counts.md
//! The counts a header carries, each with its word (ADR 0066, ADR
//! 0075).
//!
//! The review list's and the threads pane's headers count the threads
//! in scope by lifecycle: `● 2 active`, `◐ 1 resolution proposed`, and
//! `○ 1 resolved`. A zero count is left out. Header counts are passive; a
//! hidden resolved count reads dim. The words are one set:
//! [`super::header::Header`] draws them all or none, dropping them together
//! when its row is too narrow.

use crate::app::draw::header::HintOf;
use crate::app::threads::ThreadState;
use crate::app::threads::list::Counts;

/// The word after the user's `●`: the user has the last word.
const ACTIVE: &str = "active";
/// The words after `◐`.
const PROPOSED: &str = "resolution proposed";
/// The word after `○`.
const RESOLVED: &str = "resolved";

/// Passive lifecycle counts for pane headers.
pub(crate) fn passive_count_hints(counts: Counts, resolved_shown: bool) -> Vec<HintOf> {
    let mut hints = Vec::with_capacity(3);
    if counts.active > 0 {
        hints.push(HintOf::count(
            "●",
            ACTIVE,
            ThreadState::Active,
            counts.active,
            false,
            &[],
        ));
    }
    if counts.proposed > 0 {
        hints.push(HintOf::count(
            "◐",
            PROPOSED,
            ThreadState::Proposed,
            counts.proposed,
            false,
            &[],
        ));
    }
    if counts.resolved > 0 {
        hints.push(HintOf::count(
            "○",
            RESOLVED,
            ThreadState::Resolved,
            counts.resolved,
            !resolved_shown,
            &[],
        ));
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::draw::header::{Align, Header, Tone};

    fn theme() -> anyhow::Result<crate::app::draw::Theme> {
        let core = fathomable_core::theme::Theme::resolve("default-dark", |_| Ok(None))?;
        Ok(crate::app::draw::Theme::from_core(&core))
    }

    fn header(counts: Counts) -> Header {
        Header::counted(
            vec![(" Threads".to_owned(), Tone::Key)],
            passive_count_hints(counts, false),
            Align::Left,
        )
    }

    fn text(header: &Header, width: usize) -> anyhow::Result<String> {
        let line = header.line(&theme()?, width);
        Ok(line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
            .trim_end()
            .to_owned())
    }

    /// Every count carries its word while the row holds them all, and
    /// a zero count is left out.
    #[test]
    fn a_wide_header_names_every_count() -> anyhow::Result<()> {
        let counts = Counts {
            active: 2,
            proposed: 1,
            resolved: 1,
        };
        assert_eq!(
            text(&header(counts), 80)?,
            " Threads ● 2 active ◐ 1 resolution proposed ○ 1 resolved"
        );
        let quiet = Counts {
            resolved: 1,
            ..Counts::default()
        };
        assert_eq!(text(&header(quiet), 80)?, " Threads ○ 1 resolved");
        Ok(())
    }

    /// The number reads in the text colour and its word dim; a hidden
    /// count's number dims with it.
    #[test]
    fn the_number_is_brighter_than_its_word() -> anyhow::Result<()> {
        use ratatui::style::Modifier;

        let theme = theme()?;
        let counts = Counts {
            active: 2,
            resolved: 1,
            ..Counts::default()
        };
        let line = header(counts).line(&theme, 80);
        let span = |content: &str| {
            line.spans
                .iter()
                .find(|span| span.content == content)
                .map(|span| span.style)
        };
        assert_eq!(span("2"), Some(theme.text), "the number");
        assert_eq!(
            span(" active"),
            Some(theme.info.add_modifier(Modifier::DIM)),
            "the word"
        );
        assert_eq!(
            span("1"),
            Some(theme.text.add_modifier(Modifier::DIM)),
            "the hidden count's number"
        );
        Ok(())
    }

    /// When the words do not all fit they all go, the counts stay,
    /// and only then do counts drop from the end.
    #[test]
    fn a_narrow_header_drops_the_words_together() -> anyhow::Result<()> {
        let counts = Counts {
            active: 2,
            proposed: 1,
            resolved: 1,
        };
        let header = header(counts);
        assert_eq!(text(&header, 56)?, " Threads ● 2 ◐ 1 ○ 1");
        assert_eq!(text(&header, 20)?, " Threads ● 2 ◐ 1");
        Ok(())
    }

    /// Lifecycle counts are state, not click targets.
    #[test]
    fn lifecycle_counts_are_passive() {
        let counts = Counts {
            active: 2,
            proposed: 0,
            resolved: 1,
        };
        let header = header(counts);
        assert!((0..80).all(|column| header.action_at(80, column).is_none()));
    }
}
