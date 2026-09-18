//! The worktrees of the workspace in the viewer (ADR 0070): which one
//! is active, `]w` / `[w` to page through them, the re-root that makes
//! another one active, the picker a click on the branch opens, and the
//! reach and placement of threads another worktree's `HEAD` shows.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use fathomable_core::annotations::{LineHashes, Placement, ThreadId};
use fathomable_core::reach::Reach;
use fathomable_core::session::Marker;
use fathomable_core::workspace::Workspace;
use fathomable_core::worktrees::Worktree;

use crate::app::{App, PickerKind};

/// What the loop's watcher must move to after the app re-rooted or
/// the worktree set changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rewatch {
    /// The active worktree, watched recursively; `None` when unchanged.
    pub(crate) root: Option<PathBuf>,
    /// The git paths watched for the other worktrees (ADR 0070).
    pub(crate) extras: Vec<PathBuf>,
}

/// The commits another worktree's `HEAD` reached last time, and what
/// that answer was computed from, so a frame does not walk history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReachEntry {
    head: Option<String>,
    commits: BTreeSet<String>,
    reachable: HashSet<String>,
}

impl App {
    /// Whether the workspace has more than one worktree, so the viewer
    /// names the active one and pages.
    pub(crate) fn has_worktrees(&self) -> bool {
        self.worktrees.len() > 1
    }

    /// The branch (or short commit) the menu bar names while the
    /// workspace has more than one worktree.
    pub(crate) fn worktree_label(&self) -> Option<String> {
        if !self.has_worktrees() {
            return None;
        }
        self.active_worktree().map(Worktree::label)
    }

    /// The worktree the viewer shows, when git lists it.
    fn active_worktree(&self) -> Option<&Worktree> {
        let root = self.workspace.root();
        self.worktrees.iter().find(|w| w.root() == root)
    }

    /// The label of the worktree at `root`, or its last path component.
    fn label_of(&self, root: &Path) -> String {
        self.worktrees
            .iter()
            .find(|w| w.root() == root)
            .map_or_else(
                || {
                    root.file_name().map_or_else(
                        || root.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    )
                },
                Worktree::label,
            )
    }

    /// The branch on a threads pane entry (ADR 0070): the first other
    /// worktree that reaches `id` when the active one does not.
    pub(crate) fn worktree_of(&self, id: &ThreadId) -> Option<String> {
        let thread = self.thread(id)?;
        self.reach.elsewhere(thread).map(|root| self.label_of(root))
    }

    /// Where a thread another worktree shows sits in that worktree's
    /// file (ADR 0070), computed at the last reach refresh.
    pub(super) fn elsewhere_placement(&self, id: &ThreadId) -> Option<Placement> {
        self.elsewhere.get(id).copied()
    }

    /// List the worktrees again (ADR 0070): at start and whenever git
    /// metadata moves. The marker follows the roots, and the watcher is
    /// told when the paths it watches for the set changed.
    pub(crate) fn refresh_worktrees(&mut self) {
        let listed = self.workspace.worktrees();
        let paths = self.workspace.worktree_watch_paths();
        let roots_changed = listed
            .iter()
            .map(Worktree::root)
            .ne(self.worktrees.iter().map(Worktree::root));
        if paths != self.worktree_paths {
            self.worktree_paths.clone_from(&paths);
            let rewatch = self.rewatch.get_or_insert(Rewatch {
                root: None,
                extras: Vec::new(),
            });
            rewatch.extras = paths;
        }
        // The viewer's start wrote the marker; a set that changes since
        // rewrites it.
        if roots_changed && !self.worktrees.is_empty() {
            self.write_marker(&listed);
        }
        if listed != self.worktrees {
            tracing::info!(count = listed.len(), "worktrees listed");
            self.worktrees = listed;
        }
    }

    /// Write the workspace marker with the roots `listed` (ADR 0070).
    fn write_marker(&self, listed: &[Worktree]) {
        let mut roots: Vec<PathBuf> = listed.iter().map(|w| w.root().to_path_buf()).collect();
        if roots.is_empty() {
            roots.push(self.workspace.root().to_path_buf());
        }
        let marker = Marker::new(self.workspace.key().to_path_buf(), roots);
        if let Err(error) = marker.write(&self.dirs) {
            tracing::warn!(%error, "cannot write the workspace marker");
        }
    }

    /// What the loop's watcher must move to, once.
    pub(crate) fn take_rewatch(&mut self) -> Option<Rewatch> {
        self.rewatch.take()
    }

    /// The paths the watcher covers for the other worktrees, for a loop
    /// that starts watching.
    pub(crate) fn worktree_watch_paths(&self) -> &[PathBuf] {
        &self.worktree_paths
    }

    /// Whether `path`, absolute, is under one of the worktree watch
    /// paths: a `HEAD`, a ref, or the registry moving (ADR 0070).
    pub(super) fn is_worktree_path(&self, path: &Path) -> bool {
        self.worktree_paths.iter().any(|dir| path.starts_with(dir))
    }

    /// `]w` / `[w`: the next or previous worktree, wrapping (ADR 0070).
    pub(crate) fn worktree_step(&mut self, delta: isize) {
        if !self.has_worktrees() {
            self.notice("one worktree");
            return;
        }
        let count = self.worktrees.len();
        let at = self
            .worktrees
            .iter()
            .position(|w| w.root() == self.workspace.root())
            .unwrap_or(0);
        let next = (at.cast_signed() + delta)
            .rem_euclid(count.cast_signed())
            .cast_unsigned();
        let root = self.worktrees[next].root().to_path_buf();
        self.activate_worktree(&root);
    }

    /// The picker a click on the branch opens: every worktree by its
    /// label and root, the active one marked (ADR 0070).
    pub(crate) fn worktree_choices(&self) -> Vec<String> {
        self.worktrees
            .iter()
            .map(|w| {
                let mark = if w.root() == self.workspace.root() {
                    "* "
                } else {
                    "  "
                };
                format!("{mark}{}  {}", w.label(), w.root().display())
            })
            .collect()
    }

    /// The worktree picked from [`App::worktree_choices`].
    pub(crate) fn choose_worktree(&mut self, item: &str) {
        let root = item.rsplit("  ").next().map(PathBuf::from);
        if let Some(root) = root {
            self.activate_worktree(&root);
        }
    }

    /// Make the worktree at `root` active (ADR 0070): the viewer
    /// re-roots there. The current file stays open by its relative path
    /// when the worktree has it, at the same line; other documents
    /// close, and the jumplist and the recent list are cleared. Returns
    /// whether it happened; a root git does not list is refused.
    pub(crate) fn activate_worktree(&mut self, root: &Path) -> bool {
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        if root == self.workspace.root() {
            return true;
        }
        if !self.worktrees.iter().any(|w| w.root() == root) {
            self.notice(format!(
                "{} is not a worktree of this workspace",
                root.display()
            ));
            return false;
        }
        let workspace = match Workspace::discover(&root) {
            Ok(workspace) => workspace,
            Err(error) => {
                self.notice(format!("cannot open {}: {error}", root.display()));
                return false;
            }
        };
        if workspace.key() != self.workspace.key() {
            self.notice(format!("{} is another repository", root.display()));
            return false;
        }
        let keep = self.current.and_then(|i| self.docs.get(i)).map(|doc| {
            (
                doc.relative.clone(),
                doc.view.cursor_source_line().unwrap_or(1),
            )
        });
        let label = self.label_of(&root);
        self.close_popup();
        self.docs.clear();
        self.highlights.clear();
        self.current = None;
        self.recent.clear();
        self.jumplist = crate::app::jumplist::Jumplist::default();
        self.search_origin = None;
        self.thread_cursor_anchor = None;
        self.queue = fathomable_core::follow::Queue::default();
        self.tree = None;
        self.file_index.clear();
        self.all_index.clear();
        self.status = fathomable_core::status::Status::default();
        self.status_stale = false;
        self.local_thread_paths.clear();
        self.workspace = workspace;
        self.comparison =
            super::comparison::State::load(&self.dirs, &self.workspace, self.comparison.compare());
        self.rewatch = Some(Rewatch {
            root: Some(root.clone()),
            extras: self.worktree_paths.clone(),
        });
        self.record = self.record.clone().on_worktree(root.clone());
        if let Err(error) = self.record.write(&self.dirs) {
            tracing::warn!(%error, "cannot rewrite the viewer record");
        }
        self.relayout();
        self.refresh_status();
        self.refresh_comparison();
        self.refresh_reach();
        if self.sidebar.tree {
            self.ensure_tree();
        }
        match keep {
            Some((relative, line)) if root.join(&relative).is_file() => {
                self.open(&relative);
                if self.current_path() == relative {
                    let view = self.view_mut();
                    view.escape();
                    view.reveal_source_range(line, line);
                }
            }
            _ => self.relayout(),
        }
        self.notice(format!("worktree {label}"));
        tracing::info!(root = %root.display(), "worktree activated");
        true
    }

    /// The union reach of the workspace (ADR 0070): `active` is what
    /// the checkout in hand, at `head`, reaches; each other worktree's
    /// `HEAD` is walked once per `(HEAD, commits)` and cached.
    pub(super) fn reach_with_others(&mut self, head: String, active: HashSet<String>) -> Reach {
        let commits: BTreeSet<String> = self
            .store
            .iter()
            .flat_map(|store| store.commits().map(str::to_owned))
            .collect();
        let mut reach = Reach::at(head, active);
        let mut fresh: HashMap<PathBuf, ReachEntry> = HashMap::new();
        let others: Vec<Worktree> = self
            .worktrees
            .iter()
            .filter(|w| w.root() != self.workspace.root())
            .cloned()
            .collect();
        for worktree in others {
            let root = worktree.root().to_path_buf();
            let head = worktree.head().map(str::to_owned);
            // A worktree before its first commit reaches nothing.
            let Some(at) = head.clone() else {
                continue;
            };
            let reachable = match self.reach_cache.get(&root) {
                Some(entry) if entry.head == head && entry.commits == commits => {
                    entry.reachable.clone()
                }
                _ => Workspace::discover(&root)
                    .ok()
                    .and_then(|w| w.reachable(commits.iter().map(String::as_str)))
                    .unwrap_or_default(),
            };
            fresh.insert(
                root.clone(),
                ReachEntry {
                    head,
                    commits: commits.clone(),
                    reachable: reachable.clone(),
                },
            );
            reach = reach.with_worktree(root, at, reachable);
        }
        self.reach_cache = fresh;
        reach
    }

    /// Place every thread another worktree shows against that
    /// worktree's file (ADR 0070), for the pane and the list.
    pub(super) fn refresh_elsewhere(&mut self) {
        let Some(store) = self.store.as_ref() else {
            self.elsewhere.clear();
            self.local_thread_paths.clear();
            return;
        };
        let mut hashes: HashMap<PathBuf, Option<LineHashes>> = HashMap::new();
        let mut out = HashMap::new();
        for thread in store.threads() {
            let Some(root) = self.reach.elsewhere(thread) else {
                continue;
            };
            let full = root.join(thread.path());
            let hashes = hashes
                .entry(full.clone())
                .or_insert_with(|| fs::read_to_string(&full).ok().map(|t| LineHashes::of(&t)));
            let placement = match (hashes.as_ref(), thread.range()) {
                (Some(hashes), _) => crate::app::App::project_placement(
                    thread,
                    &fs::read_to_string(&full).unwrap_or_default(),
                    hashes,
                ),
                (None, Some(range)) => Placement::Detached(range),
                (None, None) => Placement::File,
            };
            out.insert(thread.id().clone(), placement);
        }
        self.elsewhere = out;
    }

    /// The `worktrees` row of `:status`: each by label and root, the
    /// active one marked.
    pub(crate) fn worktrees_row(&self) -> String {
        if self.worktrees.is_empty() {
            return "none (not a git repository)".to_owned();
        }
        self.worktrees
            .iter()
            .map(|w| {
                let mark = if w.root() == self.workspace.root() {
                    "*"
                } else {
                    ""
                };
                format!("{}{mark} {}", w.label(), w.root().display())
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A click on the menu bar's repository/worktree identity.
    pub(crate) fn pick_worktree(&mut self) {
        if !self.has_worktrees() {
            self.notice("one worktree");
            return;
        }
        self.open_picker(PickerKind::Worktree);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::annotations::{Author, Draft, LineRange, Store};
    use fathomable_core::workspace::Workspace;
    use fathomable_testing::TempDir;
    use fathomable_testing::git;

    use crate::app::input::bindings::Action;
    use crate::app::menu_bar;
    use crate::app::testing::{self, AppBuilder, screen};
    use crate::app::{App, Options, PickerKind, Popup};

    /// A repository at `main/` with `a.md` committed, and a linked
    /// worktree at `feature/` on branch `feature`.
    fn repo(name: &str) -> anyhow::Result<(TempDir, PathBuf, PathBuf)> {
        let dir = TempDir::new(&format!("worktrees-{name}"))?;
        let main = dir.0.join("main");
        fs::create_dir_all(&main)?;
        git::init(&main)?;
        git::commit_and_stage(&main, &[("a.md", "one\ntwo\nthree\n")])?;
        fs::write(main.join("a.md"), "one\ntwo\nthree\n")?;
        let feature = dir.0.join("feature");
        git::worktree_add(&main, &feature, "feature")?;
        let main = main.canonicalize()?;
        let feature = feature.canonicalize()?;
        Ok((dir, main, feature))
    }

    fn app_on(dir: &TempDir, root: &Path) -> anyhow::Result<App> {
        let store = dir.0.join("state/threads.jsonl");
        fs::create_dir_all(dir.0.join("state"))?;
        let root = root.to_path_buf();
        AppBuilder::at(&root)
            .unopened()
            .options(move |_| {
                let mut options = Options::for_test(root);
                options.store = Store::open(&store).ok();
                options
            })
            .build()
    }

    /// `]w` makes the next worktree active: the workspace re-roots, the
    /// open file follows by its relative path, and the menu bar names
    /// the repository, worktree, and file.
    #[test]
    fn paging_re_roots_the_viewer_and_keeps_the_file() -> anyhow::Result<()> {
        let (dir, main, feature) = repo("page")?;
        let mut app = app_on(&dir, &main)?;
        assert_eq!(app.worktrees.len(), 2);
        assert_eq!(app.worktree_label().as_deref(), Some("main"));
        app.toggle_menu_bar();
        app.open(Path::new("a.md"));
        app.show_tree();
        let shown = screen(&app)?;
        assert!(shown[0].contains("main · main"));
        assert!(shown[1].contains("File  a.md"));

        app.act(Action::WorktreeNext);
        assert_eq!(app.workspace().root(), feature);
        assert_eq!(app.worktree_label().as_deref(), Some("feature"));
        assert_eq!(app.current_path(), Path::new("a.md"), "the file follows");
        let shown = screen(&app)?;
        assert!(shown[0].contains("feature · feature"));
        assert!(shown[1].contains("File  a.md"));
        let identity = menu_bar::bar_identity(&app, app.width)
            .ok_or_else(|| anyhow::anyhow!("worktree identity"))?;
        testing::click(&mut app, identity.x, 0);
        assert!(matches!(
            app.popup(),
            Some(Popup::Picker(picker)) if picker.kind() == PickerKind::Worktree
        ));
        app.close_popup();
        let rewatch = app
            .take_rewatch()
            .ok_or_else(|| anyhow::anyhow!("no rewatch"))?;
        assert_eq!(rewatch.root.as_deref(), Some(feature.as_path()));

        // `[w` wraps back; a file the worktree lacks closes to the welcome.
        fs::remove_file(feature.join("a.md"))?;
        app.act(Action::WorktreePrev);
        assert_eq!(app.workspace().root(), main);
        app.act(Action::WorktreeNext);
        assert_eq!(app.current_path(), Path::new(""), "nothing open");
        Ok(())
    }

    #[test]
    fn paging_discards_checkout_local_rename_projections() -> anyhow::Result<()> {
        let (dir, main, feature) = repo("rename-projection")?;
        let store = dir.0.join("state/threads.jsonl");
        fs::create_dir_all(dir.0.join("state"))?;
        let head = Workspace::discover(&main)?
            .head_commit()
            .ok_or_else(|| anyhow::anyhow!("no main head"))?;
        let id = Store::open(&store)?.annotate(
            Draft::new(
                Author::agent("bot"),
                Path::new("a.md"),
                LineRange::new(2, 2),
                "keep the checkout path local",
            )
            .at_commit(Some(head)),
            "one\ntwo\nthree\n",
            5,
        )?;

        let mut app = app_on(&dir, &main)?;
        app.open(Path::new("a.md"));
        app.local_thread_paths
            .insert(id, PathBuf::from("renamed.md"));
        assert!(app.activate_worktree(&feature));

        assert!(app.local_thread_paths.is_empty());
        assert_eq!(app.current_path(), Path::new("a.md"));
        assert_eq!(app.marks().len(), 1);
        Ok(())
    }

    /// A thread on a commit only the feature worktree reaches shows in
    /// the main worktree's threads pane with the branch on its entry,
    /// counts in no file circle, and opening it pages there.
    #[test]
    fn a_thread_from_another_worktree_is_labelled_and_opens_there() -> anyhow::Result<()> {
        let (dir, main, feature) = repo("label")?;
        git::commit_and_stage(&feature, &[("a.md", "one\ntwo\nthree\nfour\n")])?;
        fs::write(feature.join("a.md"), "one\ntwo\nthree\nfour\n")?;
        let feature_head = Workspace::discover(&feature)?
            .head_commit()
            .ok_or_else(|| anyhow::anyhow!("no feature head"))?;
        let store = dir.0.join("state/threads.jsonl");
        fs::create_dir_all(dir.0.join("state"))?;
        let id = Store::open(&store)?.annotate(
            Draft::new(
                Author::agent("bot"),
                Path::new("a.md"),
                LineRange::new(4, 4),
                "look here",
            )
            .at_commit(Some(feature_head)),
            "one\ntwo\nthree\nfour\n",
            5,
        )?;

        let mut app = app_on(&dir, &main)?;
        let entries = app.review_entries(false);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].worktree(), Some("feature"));
        assert_eq!(
            entries[0].range(),
            Some(LineRange::new(4, 4)),
            "placed in the feature worktree's file"
        );
        assert!(
            app.file_circles().is_empty(),
            "the circles count the active worktree"
        );
        app.open(Path::new("a.md"));
        assert_eq!(app.marks().len(), 1);
        assert!(
            app.marks()[0].is_detached(),
            "the active checkout must not claim the feature-only line"
        );

        assert!(app.land_on_thread(id));
        assert_eq!(app.workspace().root(), feature, "the thread is the way in");
        assert_eq!(app.review_entries(false)[0].worktree(), None);
        Ok(())
    }

    /// One worktree: no branch in the header, and `]w` says so.
    #[test]
    fn one_worktree_pages_nowhere() -> anyhow::Result<()> {
        let dir = TempDir::new("worktrees-one")?;
        let main = dir.0.join("main");
        fs::create_dir_all(&main)?;
        git::init(&main)?;
        git::commit_and_stage(&main, &[("a.md", "one\n")])?;
        let mut app = app_on(&dir, &main.canonicalize()?)?;
        assert_eq!(app.worktree_label(), None);
        app.act(Action::WorktreeNext);
        assert_eq!(app.message(), Some("one worktree"));
        Ok(())
    }
}
