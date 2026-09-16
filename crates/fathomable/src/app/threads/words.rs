// @okf-doc: /decisions/0032-placement-and-state.md
//! The lifecycle circle shared by every thread surface.

use fathomable_core::annotations::{Placement, Thread};

use crate::app::threads::ThreadState;

/// The placement word, when the lines are not where the comment was
/// written, the state word, and whether `proposed` follows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Words {
    state: ThreadState,
}

impl Words {
    /// Words for `thread` at `placement`, known when its file is open.
    #[must_use]
    pub(crate) fn of(_placement: Option<Placement>, thread: &Thread) -> Self {
        Self {
            state: ThreadState::of(thread),
        }
    }

    /// The lifecycle kind.
    #[must_use]
    pub(crate) fn state(self) -> ThreadState {
        self.state
    }

    /// Whether the thread is resolved, however it is placed.
    #[must_use]
    pub(crate) fn is_resolved(self) -> bool {
        self.state == ThreadState::Resolved
    }

    /// The one lifecycle circle every surface draws.
    #[must_use]
    pub(crate) fn glyph(self) -> &'static str {
        match self.state {
            ThreadState::Active => "●",
            ThreadState::Proposed => "◐",
            ThreadState::Resolved => "○",
        }
    }

    /// Aggregate priority: proposed, active, then resolved.
    #[must_use]
    pub(crate) fn urgency(self) -> ThreadState {
        self.state
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
    fn the_circle_says_the_lifecycle_only() -> anyhow::Result<()> {
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
        assert_eq!((fresh.glyph(), fresh.state()), ("●", ThreadState::Active));

        store.reply(&id, Reply::new(Author::agent("bot"), 1, "answer"))?;
        let active = words(&store, anchored)?;
        assert_eq!((active.glyph(), active.state()), ("●", ThreadState::Active));
        assert_eq!(active.urgency(), fresh.urgency());

        store.reply(
            &id,
            Reply::new(Author::agent("bot"), 2, "done").proposing_resolution(),
        )?;
        let proposed = words(&store, anchored)?;
        assert_eq!(
            (proposed.glyph(), proposed.state()),
            ("◐", ThreadState::Proposed)
        );
        assert_eq!(proposed.state(), ThreadState::Proposed);
        assert!(proposed.urgency() > active.urgency());

        let detached = words(&store, Some(Placement::Detached(LineRange::new(1, 1))))?;
        assert_eq!(detached.glyph(), "◐");
        assert_eq!(detached.urgency(), proposed.urgency());

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
