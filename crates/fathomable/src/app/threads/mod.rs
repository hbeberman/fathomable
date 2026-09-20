// @okf-doc: /decisions/0013-annotation-storage-and-ux.md
//! Threads on top of the view: the store, the marks, the draft, and
//! the stubs, with the thread concept's other modules beneath.
//!
//! [`App`] keeps the [`Store`] for the workspace; every open document
//! carries the [`Mark`]s of its threads, re-located whenever the text
//! changes. The draft ([`Compose`], `draft`) starts a thread, replies to
//! one, or edits a message, written in the thread's rows; the stubs
//! (`stubs`) read a thread in place. The submodules
//! are the thread cursor (`cursor`), the threads pane (`pane`),
//! the review list (`list`), deletion, detached rows, the open thread's
//! lines (`open`), lifecycle, and summary facts.
//! All of it is plain state, tested without a terminal (ADRs 0013, 0046,
//! and 0086).

pub(crate) mod archive;
pub(crate) mod cursor;
pub(crate) mod delete;
pub(crate) mod detached;
pub(crate) mod draft;
pub(crate) mod file;
pub(crate) mod fold;
pub(crate) mod list;
pub(crate) mod list_fold;
pub(crate) mod open;
pub(crate) mod pane;
pub(crate) mod proposed;
pub(crate) mod stubs;
pub(crate) mod summary;
pub(crate) mod visibility;
pub(crate) mod words;

pub(crate) use draft::{Compose, ComposeTarget};

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::{fs, io};

use cap_std::{ambient_authority, fs::Dir};
use fathomable_core::annotations::{
    Author, ContentIdentity, Draft, FullFileDigest, Lifecycle, LineHashes, LineRange,
    MessageTarget, OriginVersion, Placement, PlacementContext, ResolutionContext, Status, Store,
    Thread, ThreadId, WorkingTreeFacts, WorkingTreeState,
};
use fathomable_core::clock::now;
use fathomable_core::context::map_context;
use fathomable_core::reanchor::{Mapping, map_range};
use fathomable_core::status::State;
use fathomable_core::workspace::Workspace;

use crate::app::App;
use crate::app::threads::words::Words;

/// A thread's status, which is its colour in the gutter, the file-threads
/// pane, and the review list (ADR 0039); where the thread is placed is
/// [`Mark::placement`]. Ordered by urgency, so the most urgent of several
/// on one row is their `max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ThreadState {
    Resolved,
    Active,
    Proposed,
}

impl ThreadState {
    pub(super) fn of(thread: &Thread) -> Self {
        match thread.lifecycle() {
            Lifecycle::Active => Self::Active,
            Lifecycle::ResolutionProposed => Self::Proposed,
            Lifecycle::Resolved => Self::Resolved,
        }
    }
}

/// One thread placed in the current text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mark {
    id: ThreadId,
    placement: Placement,
    words: Words,
}

impl Mark {
    pub(crate) fn id(&self) -> &ThreadId {
        &self.id
    }

    /// The lines the thread is drawn at; `None` for a thread on the file
    /// as a whole (ADR 0063).
    pub(crate) fn range(&self) -> Option<LineRange> {
        self.placement.range()
    }

    /// Whether the thread's lines overlap `lines`; a thread on the file
    /// as a whole covers none.
    pub(crate) fn covers(&self, lines: LineRange) -> bool {
        self.range().is_some_and(|range| overlaps(range, lines))
    }

    pub(crate) fn kind(&self) -> ThreadState {
        self.words.state()
    }

    /// The thread's words and circle (ADR 0032, ADR 0066).
    pub(crate) fn words(&self) -> Words {
        self.words
    }

    /// The one circle every surface draws for the thread (ADR 0066).
    pub(crate) fn glyph(&self) -> &'static str {
        self.words.glyph()
    }

    /// Where the thread sits in the current text.
    pub(crate) fn placement(&self) -> Placement {
        self.placement
    }

    /// Whether the annotated lines are gone: the thread is shown on a
    /// row of its own (ADR 0039), not at its last known range.
    pub(crate) fn is_detached(&self) -> bool {
        self.placement.is_detached()
    }
}

/// How a message's author reads on a row: the user by the configured
/// name (ADR 0058), or an agent by its name.
pub(crate) fn author_label(author: &Author, user: &str) -> String {
    match author {
        Author::User => user.to_owned(),
        Author::Agent { name, .. } => name.clone(),
    }
}

/// Read annotation text without allowing path resolution outside the checkout.
pub(crate) fn read_checkout_text(root: &Path, path: &Path) -> io::Result<String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "expected a repository-relative file path",
        ));
    }
    // Capability-relative resolution also confines symlinks and directory swaps;
    // canonicalizing and then reopening a pathname would leave a race.
    Dir::open_ambient_dir(root, ambient_authority())?.read_to_string(path)
}

/// Capture an agent opening comment from the bound checkout.
///
/// The observed `HEAD`, working-tree state, and content identity are
/// captured together so live-viewer and headless MCP starts use one origin
/// contract without inheriting a human comparison.
pub(crate) fn agent_start_draft(
    workspace: &mut Workspace,
    author: Author,
    path: &Path,
    range: Option<LineRange>,
    body: String,
) -> Result<(Draft, String), String> {
    const MAX_CAPTURE_ATTEMPTS: usize = 2;

    for attempt in 0..MAX_CAPTURE_ATTEMPTS {
        let observed_head = workspace.head_commit();
        let status = workspace
            .status()
            .map_err(|error| format!("cannot capture {} provenance: {error}", path.display()))?;
        let text = read_checkout_text(workspace.root(), path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if workspace.head_commit() != observed_head {
            if attempt + 1 == MAX_CAPTURE_ATTEMPTS {
                return Err(format!(
                    "checkout HEAD changed while capturing {}",
                    path.display()
                ));
            }
            continue;
        }

        let state = status.get(path).map_or_else(
            || {
                if workspace.is_git() {
                    WorkingTreeState::Clean
                } else {
                    WorkingTreeState::Added
                }
            },
            |entry| match entry.state() {
                State::Modified => WorkingTreeState::Modified,
                State::Deleted => WorkingTreeState::Deleted,
                State::Added | State::Untracked => WorkingTreeState::Added,
            },
        );
        let facts = WorkingTreeFacts::new(
            observed_head,
            state,
            Some(ContentIdentity::from_text(&text)),
            workspace.identity(),
            FullFileDigest::from_bytes(text.as_bytes()),
        );
        let draft = match range {
            Some(range) => Draft::new(author, path, range, body),
            None => Draft::on_file(author, path, body),
        }
        .with_working_tree_facts(facts);
        return Ok((draft, text));
    }
    unreachable!("capture attempts always return or retry")
}

pub(super) fn overlaps(a: LineRange, b: LineRange) -> bool {
    a.start() <= b.end() && b.start() <= a.end()
}

/// Convert a zero-based message position into its storage target.
pub(super) fn message_target(message: usize) -> MessageTarget {
    if message == 0 {
        MessageTarget::Comment
    } else {
        MessageTarget::Reply(message - 1)
    }
}

impl App {
    /// The path where this viewer projects `thread` in its active checkout.
    pub(super) fn thread_path<'a>(&'a self, thread: &'a Thread) -> &'a Path {
        if self.displayed_target_is_working_tree() {
            self.local_thread_paths
                .get(thread.id())
                .map_or_else(|| thread.path(), PathBuf::as_path)
        } else {
            thread.path()
        }
    }

    pub(super) fn thread_store_unavailable(&self) -> String {
        self.thread_store_error
            .as_ref()
            .and_then(fathomable_core::annotations::StoreError::format_mismatch)
            .map_or_else(
                || "threads unavailable; run :doctor".to_owned(),
                |mismatch| {
                    format!(
                        "threads unavailable: incompatible storage versions ({} on disk, {} expected); run :doctor",
                        mismatch.found(),
                        mismatch.expected()
                    )
                },
            )
    }

    /// Whether a new comment can currently be persisted.
    pub(crate) fn commenting_available(&self) -> bool {
        self.store.is_some() && self.store_backing != super::StoreBacking::Missing
    }

    /// Whether at least one open thread is available for direct traversal.
    pub(crate) fn has_open_threads(&self) -> bool {
        self.store
            .iter()
            .flat_map(Store::threads)
            .any(|thread| thread.status() == Status::Open && self.normal_thread(thread))
    }

    /// The store, or a status-line notice explaining why there is none.
    pub(super) fn store_mut(&mut self) -> Option<&mut Store> {
        self.observe_store_backing();
        if self.store_backing == super::StoreBacking::Observed
            && fs::symlink_metadata(&self.thread_store_path)
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
        {
            self.store_backing = super::StoreBacking::Missing;
            self.set_thread_store_degraded(Some(format!(
                "{} disappeared; keeping the last loaded board",
                self.thread_store_path.display()
            )));
        }
        if self.store_backing == super::StoreBacking::Missing {
            self.error("thread store disappeared; keeping the last loaded board read-only");
            return None;
        }
        if self.store.is_none() {
            self.error(self.thread_store_unavailable());
        }
        self.store.as_mut()
    }

    /// The thread with `id`, if the store has it.
    pub(crate) fn thread(&self, id: &ThreadId) -> Option<&Thread> {
        self.store.as_ref()?.thread(id)
    }

    /// Marks of the current document, oldest thread first.
    pub(crate) fn marks(&self) -> &[Mark] {
        self.current
            .and_then(|i| self.docs.get(i))
            .map_or(&[], |doc| doc.marks.as_slice())
    }

    /// The marks placed on lines, leaving out the detached ones, which
    /// draw on rows of their own (ADR 0039).
    pub(super) fn placed_marks(&self) -> impl Iterator<Item = &Mark> {
        self.marks().iter().filter(|mark| {
            matches!(
                mark.placement,
                Placement::Anchored(_) | Placement::Edited(_)
            )
        })
    }

    /// The most urgent mark overlapping `lines` (a rendered row can carry
    /// several source lines).
    pub(crate) fn mark_in(&self, lines: LineRange) -> Option<ThreadState> {
        self.placed_marks()
            .filter(|mark| mark.covers(lines))
            .map(Mark::kind)
            .max()
    }

    /// `(unresolved, total)` threads on the current document.
    pub(crate) fn thread_counts(&self) -> (usize, usize) {
        let unresolved = self
            .marks()
            .iter()
            .filter(|mark| matches!(mark.kind(), ThreadState::Active | ThreadState::Proposed))
            .count();
        (unresolved, self.marks().len())
    }

    /// Re-locate every thread of the document at `index` in its text.
    /// After a reload: threads whose lines stopped matching are followed
    /// through the diff from `old` to the new text and, when their lines
    /// were rewritten rather than removed, re-anchored onto the
    /// replacement and recorded as edited (ADR 0019).
    pub(super) fn remap_marks(&mut self, index: usize, old: &str) {
        let Some(doc) = self.docs.get(index) else {
            return;
        };
        let text = doc.view.text().to_owned();
        let hashes = LineHashes::of(&text);
        let path = doc.relative.clone();
        let placement =
            PlacementContext::new(OriginVersion::working_tree(self.workspace.head_commit()))
                .at_checkout(self.workspace.root().display().to_string());
        let previous: Vec<(ThreadId, Placement)> = doc
            .marks
            .iter()
            .map(|mark| (mark.id.clone(), mark.placement))
            .collect();
        let on_path: HashSet<ThreadId> = self
            .store
            .iter()
            .flat_map(Store::threads)
            .filter(|thread| self.thread_path(thread) == path)
            .map(|thread| thread.id().clone())
            .collect();
        let Some(store) = self.store_mut() else {
            return;
        };
        let stale: Vec<(ThreadId, LineRange)> = store
            .threads()
            .iter()
            .filter(|thread| on_path.contains(thread.id()))
            .filter(|thread| thread.locate_in(&hashes).is_detached())
            .filter_map(|thread| {
                // Where it sat in the old text; a thread already detached
                // there has nothing to follow.
                previous
                    .iter()
                    .find(|(id, _)| id == thread.id())
                    .filter(|(_, placement)| !placement.is_detached())
                    .and_then(|(id, placement)| Some((id.clone(), placement.range()?)))
            })
            .collect();
        let mut wrote = false;
        for (id, range) in stale {
            let target = match map_range(old, &text, range) {
                Mapping::Edited(range) | Mapping::Moved(range) => range,
                Mapping::Removed => continue,
            };
            wrote = true;
            match store.relocate(&id, target, &text, placement.clone(), now()) {
                Ok(()) => {
                    tracing::info!(%id, path = %path.display(), from = %range, to = %target, "thread re-anchored to edited lines");
                }
                Err(error) => tracing::warn!(%id, %error, "cannot re-anchor thread"),
            }
        }
        if wrote {
            self.refresh_after_thread_store_change();
        } else {
            self.reconcile_agent_activity();
            self.refresh_marks(index);
        }
    }

    pub(super) fn refresh_marks(&mut self, index: usize) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let local_paths = &self.local_thread_paths;
        let use_local_paths = self.displayed_target_is_working_tree();
        let visible: HashSet<ThreadId> = store
            .threads()
            .iter()
            .filter(|thread| self.normal_thread(thread))
            .map(|thread| thread.id().clone())
            .collect();
        let Some(doc) = self.docs.get_mut(index) else {
            return;
        };
        let text = doc.view.text().to_owned();
        let hashes = LineHashes::of(&text);
        doc.marks = store
            .threads()
            .iter()
            .filter(|thread| visible.contains(thread.id()))
            .filter(|thread| {
                let path = if use_local_paths {
                    local_paths
                        .get(thread.id())
                        .map_or_else(|| thread.path(), PathBuf::as_path)
                } else {
                    thread.path()
                };
                path == doc.relative
            })
            .map(|thread| {
                let placement = Self::project_placement(thread, &text, &hashes);
                Mark {
                    id: thread.id().clone(),
                    placement,
                    words: Words::of(Some(placement), thread),
                }
            })
            .collect();
        let detached = doc.marks.iter().filter(|m| m.is_detached()).count();
        tracing::debug!(path = %doc.relative.display(), marks = doc.marks.len(), detached, "marks refreshed");
        // The threads pane's height follows the marks (ADR 0027), and
        // the detached ones take rows of their own (ADR 0039).
        if self.current == Some(index) {
            self.place_detached_rows();
            self.place_stub_rows();
            self.scroll_tree();
        }
    }

    /// Project a thread into a displayed document without persisting an
    /// implicit relocation. The origin context is the bounded restart
    /// evidence when the stored anchor no longer locates.
    pub(crate) fn project_placement(thread: &Thread, text: &str, hashes: &LineHashes) -> Placement {
        let placement = thread.locate_in(hashes);
        if !placement.is_detached() {
            return placement;
        }
        let Some(context) = thread
            .placement_evidence()
            .context()
            .or_else(|| thread.origin().context())
        else {
            return placement;
        };
        let Some(hint) = thread.range().or_else(|| thread.origin().range()) else {
            return placement;
        };
        match map_context(context, text, hint) {
            Mapping::Moved(range) | Mapping::Edited(range) => Placement::Edited(range),
            Mapping::Removed => placement,
        }
    }

    /// Remember exact renames for this viewer without mutating the shared board.
    pub(super) fn remember_thread_moves(&mut self, moved: &impl Fn(&Path) -> Option<PathBuf>) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        let targets: Vec<(ThreadId, PathBuf)> = store
            .threads()
            .iter()
            .filter_map(|thread| {
                let path = self
                    .local_thread_paths
                    .get(thread.id())
                    .map_or_else(|| thread.path(), PathBuf::as_path);
                Some((thread.id().clone(), moved(path)?))
            })
            .collect();
        for (id, path) in &targets {
            self.local_thread_paths.insert(id.clone(), path.clone());
            tracing::info!(%id, to = %path.display(), "thread projected through local rename");
        }
    }

    /// Re-locate every loaded document's threads, after a change that
    /// may touch files other than the current one (ADR 0025).
    pub(super) fn refresh_all_marks(&mut self) {
        for index in 0..self.docs.len() {
            self.refresh_marks(index);
        }
        self.refresh_review_paths();
    }

    /// Recompute commit membership once after a store reload or viewer write.
    pub(super) fn refresh_after_thread_store_change(&mut self) {
        self.recompute_reach();
        self.refresh_all_marks();
        self.reconcile_normal_thread_cursor();
    }

    /// The document's threads in line order: by first line, then the
    /// order the store holds them (ADR 0027).
    pub(crate) fn file_threads(&self) -> Vec<ThreadId> {
        let mut marks: Vec<&Mark> = self.marks().iter().collect();
        // A thread on the file as a whole has no start and comes first.
        marks.sort_by_key(|mark| mark.range().map(|range| range.start()));
        marks.into_iter().map(|mark| mark.id().clone()).collect()
    }

    /// Every open thread in workspace order: path, file-wide threads,
    /// projected line, then stable thread id.
    pub(super) fn workspace_threads(&self) -> Vec<ThreadId> {
        let mut order: Vec<ThreadId> = self
            .store
            .iter()
            .flat_map(Store::threads)
            .filter(|thread| thread.status() == Status::Open)
            .filter(|thread| self.normal_thread_without_draft(thread))
            .map(|thread| thread.id().clone())
            .collect();
        order.sort_by(|left, right| {
            self.thread_start(left)
                .cmp(&self.thread_start(right))
                .then_with(|| left.cmp(right))
        });
        order
    }

    #[cfg(test)]
    /// `(current, total)`, 1-based, of the cursor's thread among the
    /// file's, for the pane header.
    pub(crate) fn thread_position(&self) -> Option<(usize, usize)> {
        let cursor = self.thread_cursor();
        let id = cursor.thread()?;
        let order = self.file_threads();
        let index = order.iter().position(|other| other == id)?;
        Some((index + 1, order.len()))
    }

    /// Threads whose range touches the cursor's rendered row, or the
    /// detached threads standing on it (ADR 0039).
    pub(crate) fn threads_at_cursor(&self) -> Vec<ThreadId> {
        let view = self.view();
        // On a thread's stub or expanded rows, that thread (ADR 0049).
        if let Some((stub, _, _)) = self.stub_on_row(view.cursor().row)
            && let Some(id) = stub.thread()
        {
            return vec![id.clone()];
        }
        if let Some(anchor) = view.detached_anchor_of_row(view.cursor().row) {
            return self
                .detached_marks_at(anchor)
                .map(|mark| mark.id().clone())
                .collect();
        }
        let lines = view.source_lines_of_row(view.cursor().row).or_else(|| {
            view.cursor_source_line()
                .map(|line| LineRange::new(line, line))
        });
        let Some(lines) = lines else {
            return Vec::new();
        };
        self.placed_marks()
            .filter(|mark| mark.covers(lines))
            .map(|mark| mark.id().clone())
            .collect()
    }

    /// Move the cursor to the first line of `id`, when the document has it.
    pub(super) fn goto_thread(&mut self, id: &ThreadId) {
        let Some(mark) = self.marks().iter().find(|mark| mark.id() == id) else {
            // A past thread has no mark: its stored lines are the
            // nearest thing to it (ADR 0072).
            if let Some(range) = self
                .thread(id)
                .filter(|thread| self.reach.past(thread))
                .and_then(fathomable_core::annotations::Thread::range)
            {
                self.view_mut().goto_source_line(range.start());
            }
            return;
        };
        match mark.placement() {
            Placement::Detached(_) => {
                let anchor = self.detached_anchor(mark);
                self.view_mut().goto_detached_row(anchor);
            }
            Placement::Anchored(range) | Placement::Edited(range) => {
                self.view_mut().goto_source_line(range.start());
            }
            // Its block stands above the first line (ADR 0063).
            Placement::File => self.view_mut().goto_row(0),
        }
    }

    pub(super) fn message_for(&self, id: &ThreadId, target: MessageTarget) -> Option<(&str, bool)> {
        let thread = self.thread(id)?;
        match target {
            MessageTarget::Comment => Some((thread.comment(), thread.author().is_user())),
            MessageTarget::Reply(index) => thread
                .replies()
                .get(index)
                .map(|reply| (reply.body(), reply.author().is_user())),
        }
    }

    /// Resolve `id` when open, fixing it to `HEAD` (ADR 0072), reopen it
    /// otherwise; every loaded document's marks follow.
    pub(super) fn toggle_resolved(&mut self, id: &ThreadId) {
        let head = self.workspace.head_commit();
        let root = self.workspace.root().display().to_string();
        let version = head
            .clone()
            .map_or_else(|| OriginVersion::working_tree(None), OriginVersion::commit);
        let context = ResolutionContext::new(Author::User, head.clone())
            .at_checkout(root)
            .at_version(version);
        let Some(store) = self.store_mut() else {
            return;
        };
        let open = store.thread(id).is_some_and(|t| t.status() == Status::Open);
        let result = if open {
            store.resolve_with_context(id, &context, now())
        } else {
            store.reopen(id, now())
        };
        self.refresh_after_thread_store_change();
        match result {
            Ok(()) => {
                tracing::info!(%id, resolved = open, "thread status changed");
                self.notice(if open { "resolved" } else { "reopened" });
            }
            Err(error) => self.notice(format!("cannot update thread: {error}")),
        }
    }

    /// Toggle one-shot auto-resolve for an unresolved thread.
    pub(super) fn toggle_auto_resolve(&mut self, id: &ThreadId) {
        if self
            .thread(id)
            .is_some_and(|thread| thread.lifecycle() == Lifecycle::Resolved)
        {
            self.notice("auto-resolve is unavailable on a resolved thread");
            return;
        }
        let Some(store) = self.store_mut() else {
            return;
        };
        let result = store.toggle_auto_resolve(id, now());
        self.refresh_after_thread_store_change();
        match result {
            Ok(value) => {
                tracing::info!(%id, ?value, "thread auto-resolve changed");
                self.notice(if value.is_enabled() {
                    "auto-resolve enabled"
                } else {
                    "auto-resolve disabled"
                });
            }
            Err(error) => self.notice(format!("cannot update thread: {error}")),
        }
    }
}

#[cfg(test)]
mod tests;
