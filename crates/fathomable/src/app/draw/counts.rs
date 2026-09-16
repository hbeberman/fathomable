// @okf-doc: /decisions/0075-the-header-names-its-counts.md
//! The counts a header carries, each with its word (ADR 0066, ADR
//! 0075).
//!
//! The review list's and the threads pane's headers count the threads
//! in scope by lifecycle: `● 2 active`, `◐ 1 resolution proposed`, and
//! `○ 1 resolved`. A zero count is left out. The resolved
//! count is a click on `x` and reads dim while resolved threads are
//! hidden. The words are one set: [`super::header::Header`] draws them
//! all or none, dropping them together when its row is too narrow.

use crate::app::draw::header::HintOf;
use crate::app::input::bindings::Action;
use crate::app::threads::ThreadState;
use crate::app::threads::list::Counts;

/// The word after the user's `●`: the user has the last word.
const ACTIVE: &str = "active";
/// The words after `◐`.
const PROPOSED: &str = "resolution proposed";
/// The word after `○`.
const RESOLVED: &str = "resolved";

/// The hints for `counts`: active, proposed, and resolved in
/// that order, a zero left out, the resolved count dim while
/// `resolved_shown` is false.
pub(crate) fn count_hints(counts: Counts, resolved_shown: bool) -> Vec<HintOf> {
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
            &[Action::ReviewResolved],
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
            vec![(" review threads ".to_owned(), Tone::Key)],
            count_hints(counts, false),
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
            " review threads  ● 2 active ◐ 1 resolution proposed ○ 1 resolved"
        );
        let quiet = Counts {
            resolved: 1,
            ..Counts::default()
        };
        assert_eq!(text(&header(quiet), 80)?, " review threads  ○ 1 resolved");
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
        assert_eq!(text(&header, 56)?, " review threads  ● 2 ◐ 1 ○ 1");
        assert_eq!(text(&header, 26)?, " review threads  ● 2 ◐ 1");
        Ok(())
    }

    /// A click on the resolved count runs `x` in both forms.
    #[test]
    fn the_resolved_count_takes_a_click_with_or_without_its_word() {
        let counts = Counts {
            active: 2,
            proposed: 0,
            resolved: 1,
        };
        let header = header(counts);
        let worded = " review threads  ● 2 active ○ 1 resolved";
        let at = |s: &str| fathomable_core::layout::display_width(s);
        assert_eq!(
            header.action_at(80, at(worded) - 1),
            Some(Action::ReviewResolved),
            "the word is part of the hint"
        );
        assert_eq!(
            header.action_at(80, at(" review threads  ● 2 active")),
            None,
            "the space between the hints runs nothing"
        );
        let bare = " review threads  ● 2 ○ 1";
        assert_eq!(
            header.action_at(at(bare) + 2, at(bare) - 1),
            Some(Action::ReviewResolved),
            "the bare count still takes the click"
        );
    }
}
