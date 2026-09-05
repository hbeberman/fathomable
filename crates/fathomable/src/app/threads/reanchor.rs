// @okf-doc: /decisions/0020-reanchoring-across-restarts.md
//! Re-anchoring threads edited while Fathomable was closed (ADR 0020).
//!
//! The running viewer follows edits through the reload diff (ADR 0019).
//! Between runs there is no reload, so on start each annotated file is
//! compared with its last-seen snapshot (ADR 0015), the text the reader
//! last had the thread placed in, and a thread that no longer locates is
//! followed from there. A thread the snapshot cannot place — the file has
//! none, or is too large for one — is followed through the window of
//! text it carries (ADR 0038), and a thread without a window is given
//! one once it locates.

use std::fs;
use std::path::{Path, PathBuf};

use fathomable_core::annotations::{LineRange, Store, ThreadId};
use fathomable_core::context::map_context;
use fathomable_core::reanchor::{Mapping, map_range};
use fathomable_core::seen;

use crate::app::App;
use fathomable_core::clock::now;

impl App {
    /// Follow every thread that no longer locates in its file from the
    /// file's last-seen snapshot, persisting each move as a relocation.
    ///
    /// Threads whose file cannot be read, whose lines were removed rather
    /// than rewritten, or that have neither a snapshot nor a context
    /// window stay detached.
    pub(crate) fn reanchor_from_snapshots(&mut self) {
        let (Some(store), Some(seen)) = (self.store.as_mut(), self.seen.as_ref()) else {
            return;
        };
        follow_snapshots(store, seen, self.workspace.root());
        for index in 0..self.docs.len() {
            self.refresh_marks(index);
        }
    }
}

/// The mapping behind [`App::reanchor_from_snapshots`], shared with the
/// headless `--mcp` read (ADR 0024) so an agent sees current ranges with no
/// viewer running. Returns how many threads moved.
///
/// Each file's snapshot is tried first; a thread it cannot place is
/// followed through its own context window (ADR 0038). Threads that
/// locate without a window are then given one.
pub(crate) fn follow_snapshots(store: &mut Store, seen: &seen::Store, root: &Path) -> usize {
    let mut moved = 0;
    let mut paths: Vec<PathBuf> = store
        .threads()
        .iter()
        .map(|thread| thread.path().to_path_buf())
        .collect();
    paths.sort();
    paths.dedup();
    for path in paths {
        let Ok(text) = fs::read_to_string(root.join(&path)) else {
            continue;
        };
        let snapshot = match seen.text(&path) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "cannot read last-seen snapshot");
                None
            }
        };
        let relocations: Vec<(ThreadId, LineRange, LineRange, &str)> = store
            .for_path(&path)
            .filter(|thread| thread.locate(&text).is_detached())
            .filter_map(|thread| {
                let from = thread.range();
                let through_snapshot = snapshot.as_deref().and_then(|snapshot| {
                    let placement = thread.locate(snapshot);
                    (!placement.is_detached())
                        .then(|| map_range(snapshot, &text, placement.range()))
                });
                let (mapping, source) = match through_snapshot {
                    Some(mapping @ (Mapping::Edited(_) | Mapping::Moved(_))) => {
                        (mapping, "last-seen snapshot")
                    }
                    _ => (
                        map_context(thread.context()?, &text, from),
                        "context window",
                    ),
                };
                match mapping {
                    Mapping::Edited(to) | Mapping::Moved(to) => {
                        Some((thread.id().clone(), from, to, source))
                    }
                    Mapping::Removed => None,
                }
            })
            .collect();
        for (id, from, to, source) in relocations {
            match store.relocate(&id, to, &text, now()) {
                Ok(()) => {
                    moved += 1;
                    tracing::info!(%id, path = %path.display(), %from, %to, "thread re-anchored from its {source}");
                }
                Err(error) => tracing::warn!(%id, %error, "cannot re-anchor thread"),
            }
        }
        let missing: Vec<(ThreadId, LineRange)> = store
            .for_path(&path)
            .filter(|thread| thread.context().is_none())
            .filter_map(|thread| {
                let placement = thread.locate(&text);
                (!placement.is_detached()).then(|| (thread.id().clone(), placement.range()))
            })
            .collect();
        for (id, range) in missing {
            if let Err(error) = store.record_context(&id, range, &text, now()) {
                tracing::warn!(%id, %error, "cannot record the thread's context window");
            }
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use fathomable_core::annotations::{Anchor, Author, Draft, LineRange, Store, Thread};
    use fathomable_core::context::Context;
    use fathomable_core::seen;

    use crate::app::App;
    use fathomable_testing::TempDir;

    use crate::app::testing::{self, AppBuilder};

    fn app(dir: &TempDir) -> anyhow::Result<App> {
        AppBuilder::new(dir)
            .unopened()
            .seen(dir.0.join("state/seen"))
            .build()
    }

    const ORIGINAL: &str = "one\ntwo\nthree\nfour\n";

    /// Annotate line 2 in one run, edit it while nothing runs, and start
    /// again.
    fn annotate_then_edit_offline(dir: &TempDir, edited: &str) -> anyhow::Result<App> {
        fs::write(dir.0.join("ws/a.txt"), ORIGINAL)?;
        Store::open(dir.0.join("state/threads.jsonl"))?.annotate(
            Draft::new(Author::User, Path::new("a.txt"), LineRange::new(2, 2), "hm"),
            ORIGINAL,
            1,
        )?;
        seen::Store::open(&dir.0.join("state/seen"))?.record(Path::new("a.txt"), ORIGINAL)?;
        fs::write(dir.0.join("ws/a.txt"), edited)?;
        let mut app = app(dir)?;
        app.open(Path::new("a.txt"));
        Ok(app)
    }

    #[test]
    fn thread_edited_offline_follows_through_the_snapshot() -> anyhow::Result<()> {
        let dir = testing::bare("reanchor-edited")?;
        let app = annotate_then_edit_offline(&dir, "zero\none\nTWO\nthree\nfour\n")?;
        let mark = &app.marks()[0];
        assert_eq!(mark.range(), LineRange::new(3, 3));
        assert!(mark.placement().is_edited());
        assert_eq!(
            app.thread(mark.id()).map(Thread::range),
            Some(LineRange::new(3, 3))
        );
        Ok(())
    }

    #[test]
    fn thread_removed_offline_stays_detached() -> anyhow::Result<()> {
        let dir = testing::bare("reanchor-removed")?;
        let app = annotate_then_edit_offline(&dir, "one\nthree\nfour\n")?;
        assert!(app.marks()[0].is_detached());
        Ok(())
    }

    /// Annotate line 2 with no snapshot taken, edit while nothing runs,
    /// and start again: the thread's own window is all there is (ADR 0038).
    fn annotate_without_snapshot_then_edit(dir: &TempDir, edited: &str) -> anyhow::Result<App> {
        fs::write(dir.0.join("ws/a.txt"), ORIGINAL)?;
        Store::open(dir.0.join("state/threads.jsonl"))?.annotate(
            Draft::new(Author::User, Path::new("a.txt"), LineRange::new(2, 2), "hm"),
            ORIGINAL,
            1,
        )?;
        fs::write(dir.0.join("ws/a.txt"), edited)?;
        let mut app = app(dir)?;
        app.open(Path::new("a.txt"));
        Ok(app)
    }

    #[test]
    fn without_a_snapshot_the_thread_follows_through_its_window() -> anyhow::Result<()> {
        let dir = testing::bare("reanchor-nosnap")?;
        let app = annotate_without_snapshot_then_edit(&dir, "zero\none\nTWO\nthree\nfour\n")?;
        let mark = &app.marks()[0];
        assert_eq!(mark.range(), LineRange::new(3, 3));
        assert!(mark.placement().is_edited());
        let thread = app
            .thread(mark.id())
            .ok_or_else(|| anyhow::anyhow!("thread"))?;
        assert_eq!(thread.range(), LineRange::new(3, 3));
        assert_eq!(
            thread.context().map(Context::text),
            Some("zero\none\nTWO\nthree\nfour\n".to_owned()),
            "the window follows the relocation"
        );
        Ok(())
    }

    #[test]
    fn without_a_snapshot_a_rewrite_around_the_lines_detaches() -> anyhow::Result<()> {
        let dir = testing::bare("reanchor-nosnap-rewrite")?;
        let app = annotate_without_snapshot_then_edit(&dir, "ONE\nTWO\nTHREE\nFOUR\n")?;
        assert!(app.marks()[0].is_detached());
        Ok(())
    }

    #[test]
    fn a_thread_stored_without_a_window_is_given_one_on_start() -> anyhow::Result<()> {
        let dir = testing::bare("reanchor-backfill")?;
        fs::write(dir.0.join("ws/a.txt"), ORIGINAL)?;
        fs::create_dir_all(dir.0.join("state"))?;
        let anchor = serde_json::to_string(
            &Anchor::capture(ORIGINAL, LineRange::new(2, 2))
                .ok_or_else(|| anyhow::anyhow!("in text"))?,
        )?;
        fs::write(
            dir.0.join("state/threads.jsonl"),
            format!(
                concat!(
                    r#"{{"event":"annotate","v":2,"id":"old","path":"a.txt","#,
                    r#""range":{{"start":2,"end":2}},"snippet":"two","anchor":{anchor},"#,
                    r#""created":1,"comment":"hm"}}"#,
                    "\n"
                ),
                anchor = anchor
            ),
        )?;
        drop(app(&dir)?);
        let store = Store::open(dir.0.join("state/threads.jsonl"))?;
        let thread = &store.threads()[0];
        assert_eq!(
            thread.context().map(Context::text),
            Some(ORIGINAL.to_owned())
        );
        assert_eq!(thread.updated(), 1, "the backfill is not an update");

        fs::write(dir.0.join("ws/a.txt"), "one\nTWO\nthree\nfour\n")?;
        let mut app = app(&dir)?;
        app.open(Path::new("a.txt"));
        assert!(app.marks()[0].placement().is_edited());
        Ok(())
    }
}
