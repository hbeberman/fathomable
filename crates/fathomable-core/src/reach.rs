// @okf-doc: /decisions/0072-a-resolved-thread-stays-at-its-commit.md
//! Which threads a checkout shows (ADR 0024, ADR 0072).
//!
//! A thread belongs to the commit it was written against. An open one
//! shows while that commit is `HEAD` or one of its ancestors, so it
//! follows the work through merges and rebases; a resolved one shows
//! only while that commit *is* `HEAD`, so a finished discussion drops
//! out of the file on the next commit and comes back when that commit is
//! checked out again. A thread without a commit, or a workspace without
//! git, is always shown.
//!
//! A workspace with several worktrees (ADR 0070) reaches the union:
//! [`Reach::includes`] asks about every worktree, [`Reach::here`] about
//! the checkout in hand alone, and [`Reach::elsewhere`] names the first
//! other worktree that shows a thread this one does not. [`Reach::past`]
//! names the resolved threads of earlier commits on this checkout's
//! branch: not shown in the file, listed by the review on request.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::annotations::{Status, Thread};

/// One checkout: its `HEAD` and the commits `HEAD` reaches among those
/// the store mentions.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Checkout {
    head: String,
    reachable: HashSet<String>,
}

impl Checkout {
    /// Whether this checkout shows `thread`: an open thread at a commit
    /// `HEAD` reaches, a resolved one at `HEAD` itself, or any thread
    /// without a commit.
    fn shows(&self, thread: &Thread) -> bool {
        let Some(commit) = thread.commit() else {
            return true;
        };
        if thread.status() == Status::Resolved {
            commit == self.head
        } else {
            self.reachable.contains(commit)
        }
    }

    /// Whether `thread` is resolved at a commit `HEAD` reaches but is
    /// not.
    fn past(&self, thread: &Thread) -> bool {
        thread.status() == Status::Resolved
            && thread
                .commit()
                .is_some_and(|commit| commit != self.head && self.reachable.contains(commit))
    }
}

/// Which threads the current `HEAD` shows (ADR 0024, ADR 0072).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reach {
    /// The checkout in hand; `None` shows everything.
    here: Option<Checkout>,
    /// The other worktrees, in the listing's order.
    others: Vec<(PathBuf, Checkout)>,
}

impl Reach {
    /// A reach that shows every thread: no git, or no `HEAD` yet.
    #[must_use]
    pub fn everything() -> Self {
        Self::default()
    }

    /// The reach of a checkout at `head` over `commits`, the commits
    /// `HEAD` reaches among those the store mentions; see
    /// [`Workspace::reachable`](crate::workspace::Workspace::reachable).
    #[must_use]
    pub fn at(head: impl Into<String>, commits: HashSet<String>) -> Self {
        Self {
            here: Some(Checkout {
                head: head.into(),
                reachable: commits,
            }),
            others: Vec::new(),
        }
    }

    /// Add another worktree of the workspace, at `root`, with its `HEAD`
    /// and the commits that `HEAD` reaches (ADR 0070).
    #[must_use]
    pub fn with_worktree(
        mut self,
        root: PathBuf,
        head: impl Into<String>,
        commits: HashSet<String>,
    ) -> Self {
        self.others.push((
            root,
            Checkout {
                head: head.into(),
                reachable: commits,
            },
        ));
        self
    }

    /// Whether `thread` is on the current work in any worktree.
    #[must_use]
    pub fn includes(&self, thread: &Thread) -> bool {
        self.here(thread) || self.elsewhere(thread).is_some()
    }

    /// Whether the checkout in hand shows `thread`.
    #[must_use]
    pub fn here(&self, thread: &Thread) -> bool {
        self.here
            .as_ref()
            .is_none_or(|checkout| checkout.shows(thread))
    }

    /// The first other worktree that shows `thread` when the checkout in
    /// hand does not; `None` when it does, or when none does.
    #[must_use]
    pub fn elsewhere(&self, thread: &Thread) -> Option<&Path> {
        if self.here(thread) {
            return None;
        }
        self.others
            .iter()
            .find(|(_, checkout)| checkout.shows(thread))
            .map(|(root, _)| root.as_path())
    }

    /// Whether `thread` is resolved at an earlier commit of the checkout
    /// in hand: reached by `HEAD`, not shown in the file (ADR 0072).
    #[must_use]
    pub fn past(&self, thread: &Thread) -> bool {
        self.here
            .as_ref()
            .is_some_and(|checkout| checkout.past(thread))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    use super::Reach;
    use crate::annotations::{Author, Draft, LineRange, Store, Thread};

    fn threads(
        name: &str,
        commit: Option<&str>,
    ) -> Result<(Thread, Thread), Box<dyn std::error::Error>> {
        let dir = fathomable_testing::TempDir::new(&format!("reach-{name}"))?;
        let mut store = Store::open(dir.0.join("threads.jsonl"))?;
        let draft = || {
            Draft::new(Author::User, Path::new("a.md"), LineRange::new(1, 1), "x")
                .at_commit(commit.map(str::to_owned))
        };
        let open = store.annotate(draft(), "one\n", 1)?;
        let done = store.annotate(draft(), "one\n", 2)?;
        store.resolve(&done, None, 3)?;
        let open = store.thread(&open).cloned().ok_or("thread missing")?;
        let done = store.thread(&done).cloned().ok_or("thread missing")?;
        Ok((open, done))
    }

    /// An open thread shows at `HEAD` and at its ancestors; a resolved
    /// one at `HEAD` alone, and is past at an ancestor (ADR 0072).
    #[test]
    fn a_resolved_thread_shows_at_its_commit_alone() -> Result<(), Box<dyn std::error::Error>> {
        let reach = Reach::at("head", ["head".to_owned(), "older".to_owned()].into());
        let (open_here, done_here) = threads("resolved-here", Some("head"))?;
        let (open_older, done_older) = threads("resolved-older", Some("older"))?;
        let (open_off, done_off) = threads("resolved-off", Some("elsewhere"))?;
        let (open_none, done_none) = threads("resolved-none", None)?;

        assert!(reach.here(&open_here) && reach.here(&done_here));
        assert!(reach.here(&open_older) && !reach.here(&done_older));
        assert!(!reach.here(&open_off) && !reach.here(&done_off));
        assert!(reach.here(&open_none) && reach.here(&done_none));

        assert!(reach.past(&done_older));
        assert!(!reach.past(&done_here) && !reach.past(&done_off) && !reach.past(&done_none));
        assert!(!reach.past(&open_older));
        assert!(!Reach::everything().past(&done_older));
        Ok(())
    }

    /// The checkout in hand reaches its own commits; another worktree's
    /// reach widens what shows and names where (ADR 0070), a resolved
    /// thread only at that worktree's `HEAD` (ADR 0072).
    #[test]
    fn another_worktree_widens_the_reach_and_is_named() -> Result<(), Box<dyn std::error::Error>> {
        let here: HashSet<String> = ["aaa".to_owned()].into();
        let there: HashSet<String> = ["bbb".to_owned(), "aaa".to_owned()].into();
        let reach = Reach::at("aaa", here).with_worktree(PathBuf::from("/feature"), "bbb", there);
        let (mine, mine_done) = threads("worktree-mine", Some("aaa"))?;
        let (theirs, theirs_done) = threads("worktree-theirs", Some("bbb"))?;
        let (nobody, _) = threads("worktree-nobody", Some("ccc"))?;
        let (unscoped, _) = threads("worktree-unscoped", None)?;

        assert!(reach.here(&mine) && reach.includes(&mine));
        assert_eq!(reach.elsewhere(&mine), None);
        assert!(!reach.here(&theirs) && reach.includes(&theirs));
        assert_eq!(reach.elsewhere(&theirs), Some(Path::new("/feature")));
        assert!(!reach.includes(&nobody));
        assert!(reach.here(&unscoped) && reach.elsewhere(&unscoped).is_none());
        // Resolved at this HEAD: here, and not elsewhere even though the
        // other worktree reaches the commit.
        assert!(reach.here(&mine_done) && reach.elsewhere(&mine_done).is_none());
        // Resolved at the other worktree's HEAD: shown there alone.
        assert!(!reach.here(&theirs_done));
        assert_eq!(reach.elsewhere(&theirs_done), Some(Path::new("/feature")));
        assert!(!reach.past(&theirs_done));
        Ok(())
    }
}
