//! The pickers' file index, kept across events rather than rebuilt
//! (ADR 0028).
//!
//! A walk of the tree costs a `readdir` per directory, tens of
//! milliseconds on a large repository, and every agent write used to
//! drop the index so `Space f f` paid it again. Now a path that appears
//! joins the index where the walk would have put it, a path that goes
//! leaves it, and only a change to the ignore rules or a lost-events
//! rescan walks again.

use std::cmp::Ordering;
use std::path::Path;

use fathomable_core::workspace::{EntryKind, Filter, Workspace, walk_order};

/// The files a picker offers, walked once and then patched.
#[derive(Debug)]
pub(super) struct FileIndex {
    filter: Filter,
    files: Option<Vec<String>>,
}

impl FileIndex {
    pub(super) fn new(filter: Filter) -> Self {
        Self {
            filter,
            files: None,
        }
    }

    /// The files, in walk order; walked now when not in hand.
    pub(super) fn files(&mut self, workspace: &mut Workspace) -> Vec<String> {
        if self.files.is_none() {
            let files = workspace.walk_files(self.filter);
            tracing::info!(files = files.len(), filter = ?self.filter, "indexed workspace");
            self.files = Some(files);
        }
        self.files.clone().unwrap_or_default()
    }

    /// Forget the files: the next use walks again.
    pub(super) fn clear(&mut self) {
        self.files = None;
    }

    /// Root-relative `relative` is on disk: it joins the index if it is
    /// a file the filter shows, and a directory the index knows nothing
    /// under (one that arrived whole) is walked. Nothing happens before
    /// the first walk.
    pub(super) fn seen(&mut self, workspace: &mut Workspace, relative: &Path) {
        let Some(files) = self.files.as_mut() else {
            return;
        };
        // A symlink is a file here, as the walk has it.
        let Ok(meta) = workspace.root().join(relative).symlink_metadata() else {
            return;
        };
        let kind = if meta.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        if self.filter == Filter::Visible && workspace.is_ignored(relative, kind) {
            return;
        }
        let path = relative.to_string_lossy().into_owned();
        let arrived = match kind {
            EntryKind::File => vec![path],
            EntryKind::Dir => {
                let prefix = format!("{path}/");
                if files.iter().any(|have| have.starts_with(&prefix)) {
                    return;
                }
                workspace.walk_files_under(relative, self.filter)
            }
        };
        for path in arrived {
            let at = files.partition_point(|have| walk_order(have, &path) == Ordering::Less);
            if files.get(at) != Some(&path) {
                files.insert(at, path);
            }
        }
    }

    /// Root-relative `relative` is gone: it, and everything under it,
    /// leaves the index.
    pub(super) fn removed(&mut self, relative: &Path) {
        let Some(files) = self.files.as_mut() else {
            return;
        };
        let path = relative.to_string_lossy();
        let prefix = format!("{path}/");
        files.retain(|have| *have != path && !have.starts_with(&prefix));
    }
}
