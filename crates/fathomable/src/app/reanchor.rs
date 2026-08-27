// @okf-doc: /decisions/0020-reanchoring-across-restarts.md
//! Re-anchoring threads edited while Fathomable was closed (ADR 0020).
//!
//! The running viewer follows edits through the reload diff (ADR 0019).
//! Between runs there is no reload, so on start each annotated file is
//! compared with its last-seen snapshot (ADR 0015), the text the reader
//! last had the thread placed in, and a thread that no longer locates is
//! followed from there.

use std::fs;
use std::path::{Path, PathBuf};

use fathomable_core::annotations::{LineRange, Store, ThreadId};
use fathomable_core::reanchor::{Mapping, map_range};
use fathomable_core::seen;

use super::App;
use super::threads::now;

impl App {
    /// Follow every thread that no longer locates in its file from the
    /// file's last-seen snapshot, persisting each move as a relocation.
    ///
    /// Threads without a snapshot, whose file cannot be read, or whose
    /// lines were removed rather than rewritten stay detached.
    pub(super) fn reanchor_from_snapshots(&mut self) {
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
pub(crate) fn follow_snapshots(store: &mut Store, seen: &seen::Store, root: &Path) -> usize {
    let mut moved = 0;
    {
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
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "cannot read last-seen snapshot");
                    continue;
                }
            };
            let relocations: Vec<(ThreadId, LineRange, LineRange)> = store
                .for_path(&path)
                .filter(|thread| thread.locate(&text).is_detached())
                .filter_map(|thread| {
                    let placement = thread.locate(&snapshot);
                    (!placement.is_detached()).then(|| (thread.id().clone(), placement.range()))
                })
                .filter_map(|(id, from)| match map_range(&snapshot, &text, from) {
                    Mapping::Edited(to) | Mapping::Moved(to) => Some((id, from, to)),
                    Mapping::Removed => None,
                })
                .collect();
            for (id, from, to) in relocations {
                match store.relocate(&id, to, &text, now()) {
                    Ok(()) => {
                        moved += 1;
                        tracing::info!(%id, path = %path.display(), %from, %to, "thread re-anchored from its last-seen snapshot");
                    }
                    Err(error) => tracing::warn!(%id, %error, "cannot re-anchor thread"),
                }
            }
        }
    }
    moved
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::annotations::{Draft, LineRange, Store, Thread};
    use fathomable_core::seen;
    use fathomable_core::workspace::Workspace;

    use crate::app::threads::MarkKind;
    use crate::app::{App, Options};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> std::io::Result<Self> {
            let dir = std::env::temp_dir()
                .join(format!("fathomable-reanchor-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("ws"))?;
            Ok(Self(dir))
        }

        fn app(&self) -> anyhow::Result<App> {
            let workspace = Workspace::discover(self.0.join("ws"))?;
            let store = Store::open(self.0.join("state/threads.jsonl"))?;
            let options = Options {
                store: Some(store),
                seen: Some(seen::Store::open(&self.0.join("state/seen"))?),
                ..Options::for_test(self.0.join("ws"))
            };
            Ok(App::new(workspace, 100, 30, options))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const ORIGINAL: &str = "one\ntwo\nthree\nfour\n";

    /// Annotate line 2 in one run, edit it while nothing runs, and start
    /// again.
    fn annotate_then_edit_offline(dir: &TempDir, edited: &str) -> anyhow::Result<App> {
        fs::write(dir.0.join("ws/a.txt"), ORIGINAL)?;
        Store::open(dir.0.join("state/threads.jsonl"))?.annotate(
            Draft::new(Path::new("a.txt"), LineRange::new(2, 2), "hm"),
            ORIGINAL,
            1,
        )?;
        seen::Store::open(&dir.0.join("state/seen"))?.record(Path::new("a.txt"), ORIGINAL)?;
        fs::write(dir.0.join("ws/a.txt"), edited)?;
        let mut app = dir.app()?;
        app.open(Path::new("a.txt"));
        Ok(app)
    }

    #[test]
    fn thread_edited_offline_follows_through_the_snapshot() -> anyhow::Result<()> {
        let dir = TempDir::new("edited")?;
        let app = annotate_then_edit_offline(&dir, "zero\none\nTWO\nthree\nfour\n")?;
        let mark = &app.marks()[0];
        assert_eq!(mark.range(), LineRange::new(3, 3));
        assert_eq!(mark.kind(), MarkKind::Edited);
        assert_eq!(
            app.thread(mark.id()).map(Thread::range),
            Some(LineRange::new(3, 3))
        );
        Ok(())
    }

    #[test]
    fn thread_removed_offline_stays_detached() -> anyhow::Result<()> {
        let dir = TempDir::new("removed")?;
        let app = annotate_then_edit_offline(&dir, "one\nthree\nfour\n")?;
        assert_eq!(app.marks()[0].kind(), MarkKind::Detached);
        Ok(())
    }

    #[test]
    fn without_a_snapshot_nothing_changes() -> anyhow::Result<()> {
        let dir = TempDir::new("nosnap")?;
        fs::write(dir.0.join("ws/a.txt"), ORIGINAL)?;
        Store::open(dir.0.join("state/threads.jsonl"))?.annotate(
            Draft::new(Path::new("a.txt"), LineRange::new(2, 2), "hm"),
            ORIGINAL,
            1,
        )?;
        fs::write(dir.0.join("ws/a.txt"), "one\nTWO\nthree\nfour\n")?;
        let mut app = dir.app()?;
        app.open(Path::new("a.txt"));
        assert_eq!(app.marks()[0].kind(), MarkKind::Detached);
        Ok(())
    }
}
