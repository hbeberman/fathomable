// @okf-doc: /decisions/0032-placement-and-state.md
//! The words and the circle that describe a thread (ADR 0032, ADR 0053,
//! ADR 0066).
//!
//! A [`ThreadState`] is the *state* and the one colour a thread has (ADR
//! 0039). The expanded thread's header and the review list say more: the
//! *placement* word (`detached`, `edited`) when the lines moved or went,
//! then the state word (`waiting`, `open`, `resolved`) always, then
//! `proposed` when an agent's newest reply proposes resolving, so where
//! a thread's lines are never hides what it needs. Every surface that
//! names a thread draws the same circle from the same facts
//! ([`Words::glyph`], ADR 0066): the colour is whose turn it is, the
//! fill is the lifecycle.

use fathomable_core::annotations::{Placement, Thread};

use crate::app::threads::ThreadState;

/// The placement word, when the lines are not where the comment was
/// written, the state word, and whether `proposed` follows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Words {
    placement: Option<&'static str>,
    state: ThreadState,
    proposed: bool,
    detached: bool,
}

impl Words {
    /// Words for `thread` at `placement`, known when its file is open.
    #[must_use]
    pub(crate) fn of(placement: Option<Placement>, thread: &Thread) -> Self {
        let placement_word = placement.and_then(|placement| match placement {
            Placement::Detached(_) => Some("detached"),
            Placement::Edited(_) => Some("edited"),
            Placement::File => Some("file"),
            Placement::Anchored(_) => None,
        });
        Self {
            placement: placement_word,
            state: ThreadState::of(thread),
            proposed: thread.proposes_resolution(),
            detached: placement.is_some_and(|placement| placement.is_detached()),
        }
    }

    /// `detached` or `edited`, when the lines are not where they were;
    /// `file` for a thread on the file as a whole (ADR 0063).
    #[must_use]
    pub(crate) fn placement(self) -> Option<&'static str> {
        self.placement
    }

    /// The state word's kind: waiting, open, or resolved.
    #[must_use]
    pub(crate) fn state(self) -> ThreadState {
        self.state
    }

    /// Whether `proposed` follows the state word: the thread is open and
    /// an agent's newest reply proposes resolving it (ADR 0053).
    #[must_use]
    pub(crate) fn proposed(self) -> bool {
        self.proposed
    }

    /// Whether the thread is resolved, however it is placed.
    #[must_use]
    pub(crate) fn is_resolved(self) -> bool {
        self.state == ThreadState::Resolved
    }

    /// The one circle every surface draws (ADR 0066): `?` when the lines
    /// are gone, `◐` when an agent proposes resolving, `○` once
    /// resolved, else `●`; always in the state's colour.
    #[must_use]
    pub(crate) fn glyph(self) -> &'static str {
        if self.detached {
            "?"
        } else if self.proposed {
            "◐"
        } else if self.is_resolved() {
            "○"
        } else {
            "●"
        }
    }

    /// How loudly the thread asks for the reader, for the one circle a
    /// file or a row shows for several threads: the state first, then
    /// `?`, `●`, `◐`, `○`.
    #[must_use]
    pub(crate) fn urgency(self) -> (ThreadState, u8) {
        let shape = match self.glyph() {
            "?" => 3,
            "●" => 2,
            "◐" => 1,
            _ => 0,
        };
        (self.state, shape)
    }
}

/// The status word for a kind.
#[must_use]
pub(crate) fn label(kind: ThreadState) -> &'static str {
    match kind {
        ThreadState::Waiting => "waiting",
        ThreadState::Open => "open",
        ThreadState::Resolved => "resolved",
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use fathomable_core::annotations::{Author, Draft, LineRange, Placement, Reply, Store};
    use fathomable_testing::TempDir;

    use super::Words;
    use crate::app::threads::ThreadState;

    /// One circle per lifecycle step, in the state's colour (ADR 0066).
    #[test]
    fn the_circle_says_the_lifecycle_and_the_colour_whose_turn() -> anyhow::Result<()> {
        let dir = TempDir::new("words-circle")?;
        let mut store = Store::open(dir.0.join("threads.jsonl"))?;
        let draft = Draft::new(
            Author::User,
            Path::new("a.md"),
            LineRange::new(1, 1),
            "comment",
        );
        let id = store.annotate(draft, "line\n", 0)?;
        let anchored = Some(Placement::Anchored(LineRange::new(1, 1)));
        let words = |store: &Store, placement| -> anyhow::Result<Words> {
            let thread = store
                .thread(&id)
                .ok_or_else(|| anyhow::anyhow!("the thread"))?;
            Ok(Words::of(placement, thread))
        };
        let fresh = words(&store, anchored)?;
        assert_eq!((fresh.glyph(), fresh.state()), ("●", ThreadState::Open));

        store.reply(&id, Reply::new(Author::agent("bot"), 1, "answer"))?;
        let waiting = words(&store, anchored)?;
        assert_eq!(
            (waiting.glyph(), waiting.state()),
            ("●", ThreadState::Waiting)
        );
        assert!(waiting.urgency() > fresh.urgency());

        store.reply(
            &id,
            Reply::new(Author::agent("bot"), 2, "done").proposing_resolution(),
        )?;
        let proposed = words(&store, anchored)?;
        assert_eq!(
            (proposed.glyph(), proposed.state()),
            ("◐", ThreadState::Waiting)
        );
        assert!(proposed.proposed());
        assert!(
            waiting.urgency() > proposed.urgency(),
            "an answer before a nod"
        );

        let detached = words(&store, Some(Placement::Detached(LineRange::new(1, 1))))?;
        assert_eq!(detached.glyph(), "?");
        assert_eq!(detached.placement(), Some("detached"));
        assert!(detached.urgency() > proposed.urgency());

        store.resolve(&id, None, 3)?;
        let resolved = words(&store, anchored)?;
        assert_eq!(
            (resolved.glyph(), resolved.state()),
            ("○", ThreadState::Resolved)
        );
        assert!(resolved.urgency() < fresh.urgency());
        Ok(())
    }
}
