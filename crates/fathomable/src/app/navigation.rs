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

impl App {
    /// `J`: the next comparison hunk or changed path, across files.
    pub(crate) fn hunk_next(&mut self) {
        self.step_change(true);
    }

    /// `K`: the previous comparison hunk or changed path, across files.
    pub(crate) fn hunk_prev(&mut self) {
        self.step_change(false);
    }

    /// Whether the selected comparison has a stop for `J` or `K`.
    pub(crate) fn has_change_stops(&self) -> bool {
        self.diff_mode() != DiffMode::Off && !self.comparison_status().is_empty()
    }

    fn step_change(&mut self, forward: bool) {
        if self.diff_mode() == DiffMode::Off {
            self.notice("diff mode is off");
            return;
        }
        let paths: Vec<PathBuf> = self
            .comparison_status()
            .entries()
            .iter()
            .map(|entry| entry.path().to_path_buf())
            .collect();
        if paths.is_empty() {
            self.notice("nothing in the selected comparison");
            return;
        }
        if self.step_hunk_in_file(forward) {
            return;
        }
        self.step_changed_path(&paths, forward);
    }

    fn step_hunk_in_file(&mut self, forward: bool) -> bool {
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
        let index = stepped_hunk_index(&lines, line, remembered, forward);
        let Some(index) = index else {
            return false;
        };
        self.land_on_hunk(path, &lines, index);
        true
    }

    fn step_changed_path(&mut self, paths: &[PathBuf], forward: bool) {
        let current = self.current_path();
        let (start, mut wrapped) = match paths.binary_search_by(|path| path.as_path().cmp(current))
        {
            Ok(index) if forward => ((index + 1) % paths.len(), index + 1 == paths.len()),
            Err(index) if forward => (index % paths.len(), index == paths.len()),
            Ok(index) | Err(index) => (index.checked_sub(1).unwrap_or(paths.len() - 1), index == 0),
        };
        for offset in 0..paths.len() {
            let index = if forward {
                (start + offset) % paths.len()
            } else {
                (start + paths.len() - offset) % paths.len()
            };
            if offset > 0 && ((forward && index == 0) || (!forward && index + 1 == paths.len())) {
                wrapped = true;
            }
            if self.land_on_changed_path(&paths[index], forward) {
                if wrapped {
                    self.notice(if forward {
                        "wrapped to first change"
                    } else {
                        "wrapped to last change"
                    });
                }
                return;
            }
        }

        self.notice("no comparison change can be opened");
    }

    fn land_on_changed_path(&mut self, path: &Path, forward: bool) -> bool {
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
        } else {
            let index = if forward { 0 } else { lines.len() - 1 };
            self.land_on_hunk(path.to_path_buf(), &lines, index);
        }
        true
    }

    fn land_on_hunk(&mut self, path: PathBuf, lines: &[usize], index: usize) {
        self.close_popup();
        self.open_file_view();
        self.focus = Focus::View;
        self.view_mut().goto_source_line(lines[index]);
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
}
