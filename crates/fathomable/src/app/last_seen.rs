//! Last-seen marks in the app (ADR 0015, ADR 0069). The visible file is
//! snapshotted as seen when the reader switches away from it, quits, or
//! leaves it alone for `viewer.seen-idle`; `Space d s` snapshots every
//! non-ignored text file at once, so a later `last seen · now` diff
//! shows only what came after. The store is
//! [`fathomable_core::seen`].

use std::fs;
use std::path::PathBuf;

use fathomable_core::content;
use fathomable_core::workspace::Filter;

use super::App;

impl App {
    /// Snapshot the document at `index` as seen.
    pub(super) fn mark_seen(&mut self, index: usize) {
        let Some(doc) = self.docs.get_mut(index) else {
            return;
        };
        doc.seen_dirty = false;
        if doc.deleted.is_some() {
            return;
        }
        let (Some(seen), Some(text)) = (self.seen.as_mut(), doc.document.text()) else {
            return;
        };
        match seen.record(&doc.relative, text) {
            Ok(true) => tracing::debug!(path = %doc.relative.display(), "snapshotted as seen"),
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "cannot snapshot as seen"),
        }
    }

    /// Snapshot the visible file before the session ends.
    pub(crate) fn on_quit(&mut self) {
        if let Some(index) = self.current {
            self.mark_seen(index);
        }
    }

    /// `Space d s`: snapshot every non-ignored text file as seen, the
    /// way `Space d C` checkpoints them; a file already at its snapshot,
    /// a binary file, or one over the limit is skipped. A toast counts
    /// them, and every open document's last-seen base is re-read.
    pub(crate) fn mark_all_seen(&mut self) {
        if self.seen.is_none() {
            self.notice("snapshots unavailable; see the log");
            return;
        }
        let max_bytes = self.viewer.max_file_bytes();
        let root = self.workspace.root().to_path_buf();
        let files: Vec<(PathBuf, String)> = self
            .workspace
            .walk_files(Filter::Visible)
            .into_iter()
            .map(PathBuf::from)
            .filter_map(|relative| {
                let bytes = fs::read(root.join(&relative)).ok()?;
                if u64::try_from(bytes.len()).is_ok_and(|len| len > max_bytes)
                    || content::is_binary(&bytes)
                {
                    return None;
                }
                let text = String::from_utf8(bytes).ok()?;
                Some((relative, text))
            })
            .collect();
        let Some(store) = self.seen.as_mut() else {
            return;
        };
        let mut stored = 0usize;
        let mut failed = None;
        for (path, text) in &files {
            match store.record(path, text) {
                Ok(true) => stored += 1,
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "cannot snapshot as seen");
                    failed = Some(error);
                    break;
                }
            }
        }
        for index in 0..self.docs.len() {
            self.docs[index].seen_dirty = false;
            self.refresh_base(index);
        }
        if let Some(error) = failed {
            self.notice(format!("cannot mark seen: {error}"));
            return;
        }
        self.push_toast(match stored {
            0 => "seen: nothing new".to_owned(),
            1 => "seen: 1 file".to_owned(),
            n => format!("seen: {n} files"),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use crate::app::testing::{self, press};

    /// `Space d s` snapshots every text file, counts the new ones in a
    /// toast, refreshes the open file's last-seen base so `Space d D` shows an
    /// empty diff, and finds nothing new the second time (ADR 0069).
    #[test]
    fn space_d_s_marks_every_file_seen() -> anyhow::Result<()> {
        let dir = testing::workspace("seen-all", testing::README)?;
        let root = testing::root(&dir);
        fs::write(root.join("other.md"), "# Other\n")?;
        fs::write(root.join("blob.bin"), b"\0\x01\x02")?;
        // The store sits outside the root: a walk would list it.
        let store = dir.0.join("seen");
        let mut app = testing::AppBuilder::new(&dir).seen(&store).build()?;
        assert!(!app.view().has_seen(), "nothing seen yet");

        press(&mut app, " ds");
        assert!(
            app.toasts().iter().any(|t| t.text() == "seen: 2 files"),
            "{:?}",
            app.toasts()
        );
        let store = fathomable_core::seen::Store::open(&store)?;
        assert!(store.contains(Path::new("README.md")));
        assert!(store.contains(Path::new("other.md")));
        assert!(
            !store.contains(Path::new("blob.bin")),
            "binary files are skipped"
        );
        assert!(app.view().has_seen(), "the open file's base is re-read");
        press(&mut app, " dD");
        let shown: Vec<String> = app
            .view()
            .layout()
            .lines()
            .iter()
            .map(fathomable_core::layout::Line::text)
            .collect();
        assert!(app.view().diff_view(), "Space d D opens the last-seen diff");
        assert!(
            !shown
                .iter()
                .any(|l| l.starts_with('+') || l.starts_with('-')),
            "nothing since seen: {shown:?}"
        );

        press(&mut app, " ds");
        assert!(
            app.toasts().iter().any(|t| t.text() == "seen: nothing new"),
            "{:?}",
            app.toasts()
        );
        Ok(())
    }
}
