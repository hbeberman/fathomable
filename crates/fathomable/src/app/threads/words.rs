// @okf-doc: /decisions/0032-placement-and-state.md
//! Author colour and status shape shared by every thread surface.

use fathomable_core::annotations::{AutoResolve, Placement, Thread};

use crate::app::threads::ThreadState;

/// A thread's lifecycle, latest author, and one-shot resolution permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Words {
    state: ThreadState,
    author_is_user: bool,
    auto_resolve: AutoResolve,
}

impl Words {
    /// Words for `thread` at `placement`, known when its file is open.
    #[must_use]
    pub(crate) fn of(_placement: Option<Placement>, thread: &Thread) -> Self {
        let author = thread
            .messages()
            .next_back()
            .map_or(thread.author(), |message| message.author());
        Self {
            state: ThreadState::of(thread),
            author_is_user: author.is_user(),
            auto_resolve: thread.auto_resolve(),
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

    /// Whether the latest message, including the opening comment, is human.
    pub(crate) fn author_is_user(self) -> bool {
        self.author_is_user
    }

    /// Hollow when resolved, half for a proposal or permission, full otherwise.
    #[must_use]
    pub(crate) fn glyph(self) -> &'static str {
        match self.state {
            ThreadState::Active if self.auto_resolve.is_enabled() => "◐",
            ThreadState::Active => "●",
            ThreadState::Proposed => "◐",
            ThreadState::Resolved => "○",
        }
    }

    /// Aggregate priority: proposed, permission-enabled active, active, resolved.
    #[must_use]
    pub(crate) fn urgency(self) -> (ThreadState, bool) {
        (self.state, self.auto_resolve.is_enabled())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use fathomable_core::annotations::{
        Author, AutoResolve, Draft, LineRange, MessageTarget, Placement, Reply, Store, UserSubmit,
    };
    use fathomable_testing::TempDir;

    use super::Words;
    use crate::app::threads::ThreadState;

    #[test]
    fn circle_shape_tracks_lifecycle_and_permission_but_colour_tracks_append_order()
    -> anyhow::Result<()> {
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
        assert!(fresh.author_is_user());

        store.set_auto_resolve(&id, AutoResolve::Enabled, 1)?;
        let permitted = words(&store, anchored)?;
        assert_eq!(
            (permitted.glyph(), permitted.state()),
            ("◐", ThreadState::Active)
        );
        assert!(permitted.author_is_user());
        assert!(permitted.urgency() > fresh.urgency());
        store.set_auto_resolve(&id, AutoResolve::Disabled, 1)?;
        assert_eq!(words(&store, anchored)?.glyph(), "●");

        store.reply(&id, Reply::new(Author::agent("bot"), 1, "answer"))?;
        let active = words(&store, anchored)?;
        assert_eq!((active.glyph(), active.state()), ("●", ThreadState::Active));
        assert!(!active.author_is_user());
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
        assert!(!proposed.author_is_user());
        assert!(proposed.urgency() > active.urgency());

        let detached = words(&store, Some(Placement::Detached(LineRange::new(1, 1))))?;
        assert_eq!(detached.glyph(), "◐");
        assert_eq!(detached.urgency(), proposed.urgency());
        assert_eq!(detached, proposed);

        store.resolve(&id, None, 3)?;
        let resolved = words(&store, anchored)?;
        assert_eq!(
            (resolved.glyph(), resolved.state()),
            ("○", ThreadState::Resolved)
        );
        assert!(resolved.urgency() < fresh.urgency());
        assert!(!resolved.author_is_user());

        store.reopen(&id, 4)?;
        store.edit_user(
            &id,
            MessageTarget::Comment,
            "edited opening",
            5,
            UserSubmit::Normal,
        )?;
        let reopened = words(&store, anchored)?;
        assert_eq!(
            (reopened.glyph(), reopened.state()),
            ("●", ThreadState::Active)
        );
        assert!(
            !reopened.author_is_user(),
            "editing an older message does not reorder it"
        );

        store.reply_user(&id, 6, "please finish", UserSubmit::EnableAutoResolve)?;
        let permitted = words(&store, anchored)?;
        assert_eq!(
            (permitted.glyph(), permitted.state()),
            ("◐", ThreadState::Active)
        );
        assert!(permitted.author_is_user());
        Ok(())
    }
}
