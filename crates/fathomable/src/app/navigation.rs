// @okf-doc: /decisions/0090-direct-workspace-navigation.md
//! Direct workspace-wide comparison navigation.

use std::path::{Path, PathBuf};

use fathomable_core::config::DiffMode;

use super::{App, Focus};

/// The exact comparison stop reached by the last traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChangeStop {
    path: PathBuf,
    hunk: Option<usize>,
    row: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Previous,
    Next,
}

impl Direction {
    const fn is_next(self) -> bool {
        matches!(self, Self::Next)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PathLanding {
    TraversalEdge,
    FirstHunk,
}

impl App {
    /// `J` or Shift-Down: the next comparison hunk or changed path.
    pub(crate) fn hunk_next(&mut self) {
        self.step_change(Direction::Next);
    }

    /// `K` or Shift-Up: the previous comparison hunk or changed path.
    pub(crate) fn hunk_prev(&mut self) {
        self.step_change(Direction::Previous);
    }

    /// `L` or Shift-Right: the next changed file, at its first diff.
    pub(crate) fn changed_file_next(&mut self) {
        self.step_changed_file(Direction::Next);
    }

    /// `H` or Shift-Left: the previous changed file, at its first diff.
    pub(crate) fn changed_file_prev(&mut self) {
        self.step_changed_file(Direction::Previous);
    }

    /// Whether the selected comparison has a traversable diff stop.
    pub(crate) fn has_change_stops(&self) -> bool {
        self.diff_mode() != DiffMode::Off && !self.comparison_status().is_empty()
    }

    fn comparison_paths(&mut self) -> Option<Vec<PathBuf>> {
        if self.diff_mode() == DiffMode::Off {
            self.notice("diff mode is off");
            return None;
        }
        let paths: Vec<PathBuf> = self
            .comparison_status()
            .entries()
            .iter()
            .map(|entry| entry.path().to_path_buf())
            .collect();
        if paths.is_empty() {
            self.notice("nothing in the selected comparison");
            return None;
        }
        Some(paths)
    }

    fn step_change(&mut self, direction: Direction) {
        let Some(paths) = self.comparison_paths() else {
            return;
        };
        if self.step_hunk_in_file(direction) {
            return;
        }
        self.step_changed_path(&paths, direction, PathLanding::TraversalEdge);
    }

    fn step_changed_file(&mut self, direction: Direction) {
        let Some(paths) = self.comparison_paths() else {
            return;
        };
        self.step_changed_path(&paths, direction, PathLanding::FirstHunk);
    }

    fn step_hunk_in_file(&mut self, direction: Direction) -> bool {
        let path = self.current_path().to_path_buf();
        if self.comparison_status().get(&path).is_none() {
            return false;
        }
        let lines = self.view().hunk_target_lines();
        if lines.is_empty() {
            return false;
        }
        let current_row = self.view().cursor().row;
        let remembered = self.change_stop.as_ref().and_then(|stop| {
            (stop.path == path && stop.row == current_row)
                .then_some(stop.hunk)
                .flatten()
                .filter(|index| lines.get(*index).is_some())
        });
        let line = self.view().cursor_source_line().unwrap_or(0);
        let index = stepped_hunk_index(&lines, line, remembered, direction.is_next());
        let Some(index) = index else {
            return false;
        };
        self.land_on_hunk(path, &lines, index);
        true
    }

    fn step_changed_path(&mut self, paths: &[PathBuf], direction: Direction, landing: PathLanding) {
        let current = self.current_path();
        let (start, mut wrapped) = match paths.binary_search_by(|path| path.as_path().cmp(current))
        {
            Ok(index) if direction.is_next() => {
                ((index + 1) % paths.len(), index + 1 == paths.len())
            }
            Err(index) if direction.is_next() => (index % paths.len(), index == paths.len()),
            Ok(index) | Err(index) => (index.checked_sub(1).unwrap_or(paths.len() - 1), index == 0),
        };
        for offset in 0..paths.len() {
            let index = if direction.is_next() {
                (start + offset) % paths.len()
            } else {
                (start + paths.len() - offset) % paths.len()
            };
            if offset > 0
                && ((direction.is_next() && index == 0)
                    || (!direction.is_next() && index + 1 == paths.len()))
            {
                wrapped = true;
            }
            if self.land_on_changed_path(&paths[index], direction, landing) {
                if wrapped {
                    self.notice(match (direction, landing) {
                        (Direction::Next, PathLanding::TraversalEdge) => "wrapped to first change",
                        (Direction::Previous, PathLanding::TraversalEdge) => {
                            "wrapped to last change"
                        }
                        (Direction::Next, PathLanding::FirstHunk) => {
                            "wrapped to first changed file"
                        }
                        (Direction::Previous, PathLanding::FirstHunk) => {
                            "wrapped to last changed file"
                        }
                    });
                }
                return;
            }
        }

        self.notice("no comparison change can be opened");
    }

    fn land_on_changed_path(
        &mut self,
        path: &Path,
        direction: Direction,
        landing: PathLanding,
    ) -> bool {
        self.close_popup();
        self.open_file_view();
        if self.current_path() != path {
            self.open(path);
        }
        if self.current_path() != path {
            return false;
        }
        self.focus = Focus::View;
        let lines = self.view().hunk_target_lines();
        if lines.is_empty() {
            self.view_mut().goto_source_line(1);
            self.change_stop = Some(ChangeStop {
                path: path.to_path_buf(),
                hunk: None,
                row: self.view().cursor().row,
            });
            self.synchronize_tree_to(path);
        } else {
            let index = if landing == PathLanding::FirstHunk || direction.is_next() {
                0
            } else {
                lines.len() - 1
            };
            self.land_on_hunk(path.to_path_buf(), &lines, index);
        }
        true
    }

    fn land_on_hunk(&mut self, path: PathBuf, lines: &[usize], index: usize) {
        self.close_popup();
        self.open_file_view();
        self.focus = Focus::View;
        self.view_mut().goto_source_line(lines[index]);
        self.synchronize_tree_to(&path);
        self.change_stop = Some(ChangeStop {
            path,
            hunk: Some(index),
            row: self.view().cursor().row,
        });
    }
}

fn stepped_hunk_index(
    lines: &[usize],
    current_line: usize,
    remembered: Option<usize>,
    forward: bool,
) -> Option<usize> {
    if let Some(index) = remembered {
        if forward {
            index.checked_add(1).filter(|next| *next < lines.len())
        } else {
            index.checked_sub(1)
        }
    } else if forward {
        lines.iter().position(|target| *target > current_line)
    } else {
        lines.iter().rposition(|target| *target < current_line)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use anyhow::Context;
    use fathomable_core::annotations::{Author, Draft, LineRange, Store};
    use fathomable_core::tree::{Rule, Tree};
    use fathomable_testing::{TempDir, git};

    use super::stepped_hunk_index;
    use crate::app::testing::AppBuilder;
    use crate::app::{Focus, testing};

    #[test]
    fn exact_hunk_index_keeps_equal_target_lines_distinct() {
        let lines = [4, 4, 9];
        assert_eq!(stepped_hunk_index(&lines, 4, None, true), Some(2));
        assert_eq!(stepped_hunk_index(&lines, 4, Some(0), true), Some(1));
        assert_eq!(stepped_hunk_index(&lines, 4, Some(1), false), Some(0));
    }

    #[test]
    fn metadata_only_path_is_one_comparison_stop() -> anyhow::Result<()> {
        let dir = TempDir::new("navigation-mode-only")?;
        let root = &dir.0;
        fs::write(root.join("README.md"), "one\ntwo\nthree\n")?;
        git::init(root)?;
        git::commit_and_stage(root, &[("README.md", "one\ntwo\nthree\n")])?;
        let mut permissions = fs::metadata(root.join("README.md"))?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(root.join("README.md"), permissions)?;

        let mut app = AppBuilder::at(root).source_view().build()?;
        assert_eq!(app.comparison_status().entries().len(), 1);
        assert!(app.view().hunk_target_lines().is_empty());
        app.view_mut().goto_source_line(3);
        app.hunk_next();
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.view().cursor_source_line(), Some(1));
        assert_eq!(app.message(), Some("wrapped to first change"));
        assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), None);
        Ok(())
    }

    #[test]
    fn same_file_hunk_landing_opens_and_focuses_file() -> anyhow::Result<()> {
        let base = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let dir = testing::workspace("navigation-same-file", &base)?;
        let root = testing::root(&dir);
        git::init(&root)?;
        git::commit_and_stage(&root, &[("README.md", &base)])?;
        let changed = base
            .replace("line 5", "changed 5")
            .replace("line 15", "changed 15");
        fs::write(root.join("README.md"), changed)?;

        let mut app = AppBuilder::new(&dir).source_view().build()?;
        app.view_mut().goto_source_line(1);
        app.open_review();
        assert!(app.review_list().is_open());

        app.hunk_next();
        assert_eq!(app.focus(), Focus::View);
        assert!(!app.review_list().is_open());
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.view().cursor_source_line(), Some(5));
        Ok(())
    }

    #[test]
    fn changed_file_navigation_always_lands_on_the_first_hunk() -> anyhow::Result<()> {
        let base = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let dir = testing::workspace("navigation-changed-files", &base)?;
        let root = testing::root(&dir);
        fs::write(root.join("z.txt"), &base)?;
        git::init(&root)?;
        git::commit_and_stage(&root, &[("README.md", &base), ("z.txt", &base)])?;
        let changed = base
            .replace("line 5", "changed 5")
            .replace("line 15", "changed 15");
        fs::write(root.join("README.md"), &changed)?;
        fs::write(root.join("z.txt"), &changed)?;

        let mut app = AppBuilder::new(&dir).source_view().build()?;
        assert_eq!(app.view().hunk_target_lines(), vec![5, 15]);
        app.view_mut().goto_source_line(15);

        app.changed_file_next();
        assert_eq!(app.current_path(), Path::new("z.txt"));
        assert_eq!(app.view().cursor_source_line(), Some(5));

        app.changed_file_prev();
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.view().cursor_source_line(), Some(5));
        Ok(())
    }

    #[test]
    fn unreadable_comparison_content_keeps_a_path_stop() -> anyhow::Result<()> {
        let dir = TempDir::new("navigation-unavailable-content")?;
        let root = &dir.0;
        fs::write(root.join("README.md"), "unchanged\n")?;
        git::init(root)?;
        git::commit_and_stage(root, &[("README.md", "unchanged\n")])?;
        fs::write(root.join("unavailable.txt"), [0xff, 0xfe])?;

        let mut app = AppBuilder::at(root).unopened().build()?;
        assert_eq!(app.comparison_status().entries().len(), 1);
        app.hunk_next();
        assert_eq!(app.current_path(), Path::new("unavailable.txt"));
        assert!(app.info().is_some(), "unavailable text uses file info");
        assert_eq!(app.change_stop.as_ref().and_then(|stop| stop.hunk), None);
        assert!(
            app.info()
                .is_some_and(|info| info.notice.join(" ").contains("not UTF-8")),
            "{:?}",
            app.info()
        );
        Ok(())
    }

    #[test]
    fn comparison_navigation_reveals_nested_destination_without_taking_files_focus()
    -> anyhow::Result<()> {
        let dir = TempDir::new("navigation-tree-nested")?;
        let root = &dir.0;
        fs::create_dir_all(root.join("docs/deep"))?;
        fs::write(root.join("README.md"), "readme\n")?;
        fs::write(root.join("docs/deep/target.md"), "old\n")?;
        git::init(root)?;
        git::commit_and_stage(
            root,
            &[("README.md", "readme\n"), ("docs/deep/target.md", "old\n")],
        )?;
        fs::write(root.join("docs/deep/target.md"), "new\n")?;

        let mut app = AppBuilder::at(root).source_view().build()?;
        app.show_tree();
        assert_eq!(app.focus(), Focus::Tree);
        assert!(
            app.tree()
                .is_some_and(|tree| !tree.contains(Path::new("docs/deep/target.md")))
        );

        testing::press(&mut app, "J");

        assert_eq!(app.current_path(), Path::new("docs/deep/target.md"));
        assert_eq!(app.focus(), Focus::View);
        let tree = app.tree().context("visible files tree")?;
        assert_eq!(
            tree.current().map(fathomable_core::tree::Row::path),
            Some(Path::new("docs/deep/target.md"))
        );
        for path in ["docs", "docs/deep"] {
            assert!(
                tree.rows()
                    .iter()
                    .any(|row| row.path() == Path::new(path) && row.expanded()),
                "{path} should be expanded"
            );
        }

        app.toggle_tree_focus();
        testing::press(&mut app, "hh");
        app.refresh_review_paths();
        let tree = app.tree().context("visible files tree")?;
        assert_eq!(
            tree.current().map(fathomable_core::tree::Row::path),
            Some(Path::new("docs/deep"))
        );
        assert!(
            tree.current().is_some_and(|row| !row.expanded()),
            "manual collapse must survive passive refresh"
        );
        assert!(!tree.contains(Path::new("docs/deep/target.md")));
        Ok(())
    }

    #[test]
    fn hidden_files_retains_filtered_destination_and_centers_it_when_admitted() -> anyhow::Result<()>
    {
        let dir = TempDir::new("navigation-tree-hidden")?;
        let root = &dir.0;
        let mut files = vec![("README.md".to_owned(), "readme\n".to_owned())];
        for index in 0..=40 {
            files.push((format!("{index:02}.md"), format!("old {index}\n")));
        }
        for (path, text) in &files {
            fs::write(root.join(path), text)?;
        }
        git::init(root)?;
        let refs: Vec<_> = files
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect();
        git::commit_and_stage(root, &refs)?;
        fs::write(root.join("20.md"), "new 20\n")?;

        let mut app = AppBuilder::at(root).source_view().build()?;
        app.resize(100, 12);
        app.show_tree();
        app.toggle_tree_shown();
        assert!(!app.sidebar.tree);

        testing::press(&mut app, "J");
        assert_eq!(app.current_path(), Path::new("20.md"));
        assert_eq!(app.focus(), Focus::View);
        app.toggle_tree_shown();

        let tree = app.tree().context("reopened files tree")?;
        let cursor = tree.cursor();
        assert_eq!(
            tree.current().map(fathomable_core::tree::Row::path),
            Some(Path::new("20.md"))
        );
        let body_rows = app.tree_rows().saturating_sub(1).max(1);
        assert_eq!(cursor - app.tree_scroll(), body_rows / 2);

        app.open(Path::new("README.md"));
        app.files_toggle(Rule::Reviews);
        app.toggle_tree_shown();
        testing::press(&mut app, "J");
        app.toggle_tree_shown();
        assert!(
            app.tree()
                .is_some_and(|tree| !tree.contains(Path::new("20.md"))),
            "the active filter must not fabricate the destination"
        );

        app.files_toggle(Rule::Reviews);
        assert_eq!(
            app.tree()
                .and_then(Tree::current)
                .map(fathomable_core::tree::Row::path),
            Some(Path::new("20.md")),
            "removing the filter retries the remembered destination"
        );
        assert_eq!(app.focus(), Focus::View);
        Ok(())
    }

    #[test]
    fn source_thread_navigation_reveals_nested_file_and_keeps_file_focus() -> anyhow::Result<()> {
        let dir = testing::workspace("navigation-thread-source", testing::README)?;
        let root = testing::root(&dir);
        fs::create_dir_all(root.join("docs/deep"))?;
        fs::write(root.join("docs/deep/guide.md"), "guide\n")?;
        let mut store = Store::open(testing::store_path(&dir))?;
        store.annotate(
            testing::at_working_tree(
                &root,
                Draft::new(
                    Author::agent("reviewer"),
                    Path::new("README.md"),
                    LineRange::new(1, 1),
                    "readme",
                ),
                testing::README,
            )?,
            testing::README,
            1,
        )?;
        store.annotate(
            testing::at_working_tree(
                &root,
                Draft::new(
                    Author::agent("reviewer"),
                    Path::new("docs/deep/guide.md"),
                    LineRange::new(1, 1),
                    "guide",
                ),
                "guide\n",
            )?,
            "guide\n",
            1,
        )?;

        let mut app = AppBuilder::new(&dir).source_view().build()?;
        app.show_tree();
        app.open_review();
        assert_eq!(app.focus(), Focus::Review);

        testing::press_key(&mut app, crossterm::event::KeyCode::Tab);

        assert_eq!(app.current_path(), Path::new("docs/deep/guide.md"));
        assert_eq!(app.focus(), Focus::View);
        assert!(!app.review_list().is_open());
        let tree = app.tree().context("visible files tree")?;
        assert_eq!(
            tree.current().map(fathomable_core::tree::Row::path),
            Some(Path::new("docs/deep/guide.md"))
        );
        for path in ["docs", "docs/deep"] {
            assert!(
                tree.rows()
                    .iter()
                    .any(|row| row.path() == Path::new(path) && row.expanded()),
                "{path} should be expanded"
            );
        }
        Ok(())
    }

    #[test]
    fn review_fallback_reveals_thread_file_after_hidden_files_reopens() -> anyhow::Result<()> {
        let base = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let dir = testing::workspace("navigation-thread-fallback", &base)?;
        let root = testing::root(&dir);
        fs::write(root.join("binary.bin"), [0, 1, 2])?;
        git::init(&root)?;
        git::commit_and_stage(
            &root,
            &[("README.md", &base), ("binary.bin", "\0\u{1}\u{2}")],
        )?;
        fs::write(
            root.join("README.md"),
            base.replace("line 5", "changed 5")
                .replace("line 15", "changed 15"),
        )?;
        let mut store = Store::open(testing::store_path(&dir))?;
        store.annotate(
            Draft::new(
                Author::agent("reviewer"),
                Path::new("binary.bin"),
                LineRange::new(1, 1),
                "binary",
            ),
            "source\n",
            1,
        )?;

        let mut app = AppBuilder::new(&dir).source_view().build()?;
        app.show_tree();
        app.toggle_tree_shown();
        app.open_review();
        assert_eq!(app.focus(), Focus::Review);

        testing::press_key(&mut app, crossterm::event::KeyCode::Tab);

        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.focus(), Focus::Review);
        assert!(app.review_list().is_open());
        app.toggle_tree_shown();
        assert_eq!(app.focus(), Focus::Review);
        assert_eq!(
            app.tree()
                .and_then(Tree::current)
                .map(fathomable_core::tree::Row::path),
            Some(Path::new("binary.bin"))
        );

        testing::press(&mut app, "J");
        assert_eq!(app.current_path(), Path::new("README.md"));
        assert_eq!(app.focus(), Focus::View);
        assert_eq!(
            app.tree()
                .and_then(Tree::current)
                .map(fathomable_core::tree::Row::path),
            Some(Path::new("README.md")),
            "same-file hunk landing replaces the fallback destination"
        );
        Ok(())
    }
}
