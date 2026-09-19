// @okf-doc: /decisions/0012-workspace-mode.md
//! Bounded, cancellable file discovery for the two workspace pickers.

use std::path::{Path, PathBuf};

use fathomable_core::config::LimitsConfig;
use fathomable_core::workspace::{Cancellation, EntryKind, Filter, Workspace, walk_order};

use super::background::Worker;

#[derive(Debug)]
struct Request {
    root: PathBuf,
    filter: Filter,
    limits: LimitsConfig,
}

#[derive(Debug)]
struct Indexed {
    files: Vec<String>,
    incomplete: Option<String>,
}

fn discover(request: Request, cancellation: Cancellation) -> Result<Indexed, String> {
    let mut workspace = Workspace::discover(request.root).map_err(|error| error.to_string())?;
    workspace.set_limits(request.limits);
    workspace.set_cancellation(cancellation);
    let found = workspace.discover_files(request.filter);
    Ok(Indexed {
        files: found.paths().to_vec(),
        incomplete: found.incomplete().map(ToString::to_string),
    })
}

/// The files a picker offers, with an explicit incomplete-discovery reason.
#[derive(Debug)]
pub(super) struct FileIndex {
    filter: Filter,
    files: Option<Vec<String>>,
    incomplete: Option<String>,
    worker: Worker<Request, Result<Indexed, String>>,
}

impl FileIndex {
    pub(super) fn new(filter: Filter) -> Self {
        Self {
            filter,
            files: None,
            incomplete: None,
            worker: Worker::new(discover),
        }
    }

    pub(super) fn files(&mut self, workspace: &mut Workspace) -> Vec<String> {
        if self.files.is_none() && !self.worker.pending() {
            self.incomplete = Some("scanning files; coverage is incomplete".to_owned());
            self.files = Some(Vec::new());
            if let Err(error) = self.worker.submit(Request {
                root: workspace.root().to_path_buf(),
                filter: self.filter,
                limits: workspace.limits().clone(),
            }) {
                self.incomplete = Some(format!("cannot start file discovery: {error}"));
            }
        }
        self.files.clone().unwrap_or_default()
    }

    pub(super) fn incomplete(&self) -> Option<&str> {
        self.incomplete.as_deref()
    }

    pub(super) fn pending(&self) -> bool {
        self.worker.pending()
    }

    pub(super) fn poll(&mut self) -> bool {
        match self.worker.poll() {
            Ok(Some(Ok(index))) => {
                self.files = Some(index.files);
                self.incomplete = index.incomplete;
                true
            }
            Ok(Some(Err(error))) => {
                self.incomplete = Some(error);
                true
            }
            Ok(None) => false,
            Err(error) => {
                self.incomplete = Some(error.to_string());
                true
            }
        }
    }

    pub(super) fn clear(&mut self) {
        self.worker.cancel();
        self.files = None;
        self.incomplete = None;
    }

    pub(super) fn seen(&mut self, workspace: &mut Workspace, relative: &Path) {
        if self.files.is_none() {
            return;
        }
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
        if kind == EntryKind::Dir || self.worker.pending() {
            self.clear();
            let _ = self.files(workspace);
            return;
        }
        let path = relative.to_string_lossy().into_owned();
        if let Some(files) = &mut self.files {
            let at = files.partition_point(|have| walk_order(have, &path).is_lt());
            if files.get(at) == Some(&path) {
                return;
            }
            if files.len() >= workspace.limits().retained_paths {
                self.incomplete = Some("file index limited by retained-path budget".to_owned());
            } else {
                files.insert(at, path);
            }
        }
    }

    pub(super) fn removed(&mut self, workspace: &mut Workspace, relative: &Path) {
        let Some(files) = self.files.as_mut() else {
            return;
        };
        files.retain(|path| !Path::new(path).starts_with(relative));
        if self.worker.pending() {
            self.clear();
            let _ = self.files(workspace);
        }
    }
}
