// @okf-doc: /decisions/0090-direct-workspace-navigation.md
//! Direct workspace-wide comparison navigation.

use std::ops::Range;
use std::path::{Path, PathBuf};

use fathomable_core::config::DiffMode;
use fathomable_core::diff::Hunk;

use super::{App, Focus};
use crate::app::view::ProjectedOldSeat;

/// The exact comparison stop reached by the last traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ChangeStop {
    path: PathBuf,
    hunk: Option<usize>,
    old: Range<usize>,
    new: Range<usize>,
}

/// Authority to restore a deliberate comparison placement after relayout.
#[derive(Debug)]
pub(super) struct ChangePlacement;

/// A jumplist seat on one exact old-side row in a projected Normal deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComparisonPosition {
    hunk: usize,
    old: Range<usize>,
    new: Range<usize>,
    seat: ProjectedOldSeat,
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
    /// Capture an exact old-side comparison seat for logical history.
    pub(crate) fn comparison_history_position(&self) -> Option<ComparisonPosition> {
        let seat = self.view().projected_old_seat()?;
        let hunks = self.view().hunks();
        let row = self.view().cursor().row;
        let entire_deletion = self
            .current
            .and_then(|current| self.docs.get(current))
            .is_some_and(|doc| doc.deleted == Some(super::Deleted::ComparisonBase));
        let index = self
            .change_stop
            .as_ref()
            .and_then(|stop| {
                let index = stop.hunk?;
                (stop.path == self.current_path()
                    && hunks.get(index).is_some_and(|candidate| {
                        candidate.old_range() == stop.old
                            && candidate.new_range() == stop.new
                            && self
                                .view()
                                .hunk_rows(candidate, entire_deletion)
                                .is_some_and(|rows| rows.contains(&row))
                    }))
                .then_some(index)
            })
            .or_else(|| rendered_hunk_index(self.view(), &hunks, row, entire_deletion))?;
        let hunk = hunks.get(index)?;
        Some(ComparisonPosition {
            hunk: index,
            old: hunk.old_range(),
            new: hunk.new_range(),
            seat,
        })
    }

    /// Rebuild and restore an exact old-side comparison seat from history.
    pub(crate) fn restore_comparison_history_position(
        &mut self,
        position: &ComparisonPosition,
    ) -> bool {
        let hunks = self.view().hunks();
        let Some((index, hunk)) = hunks
            .get(position.hunk)
            .filter(|hunk| hunk.old_range() == position.old && hunk.new_range() == position.new)
            .map(|hunk| (position.hunk, hunk))
            .or_else(|| {
                hunks.iter().enumerate().find(|(_, hunk)| {
                    hunk.old_range() == position.old && hunk.new_range() == position.new
                })
            })
        else {
            return false;
        };
        let entire_deletion = self
            .current
            .and_then(|current| self.docs.get(current))
            .is_some_and(|doc| doc.deleted == Some(super::Deleted::ComparisonBase));
        self.view_mut().show_normal_deletion(hunk, entire_deletion);
        if !self.view_mut().goto_projected_old_seat(position.seat) {
            return false;
        }
        self.change_stop = Some(ChangeStop {
            path: self.current_path().to_path_buf(),
            hunk: Some(index),
            old: hunk.old_range(),
            new: hunk.new_range(),
        });
        self.change_placement = None;
        true
    }

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
        let hunks = self.view().hunks();
        if hunks.is_empty() {
            return false;
        }
        let lines: Vec<usize> = hunks
            .iter()
            .map(|hunk| hunk.target_line(self.view().index().line_count()))
            .collect();
        let current_row = self.view().cursor().row;
        let entire_deletion = self
            .current
            .and_then(|current| self.docs.get(current))
            .is_some_and(|doc| doc.deleted == Some(super::Deleted::ComparisonBase));
        let remembered = self
            .change_stop
            .as_ref()
            .and_then(|stop| {
                let index = (stop.path == path).then_some(stop.hunk).flatten()?;
                (hunks.get(index).is_some_and(|hunk| {
                    hunk.old_range() == stop.old && hunk.new_range() == stop.new
                }) && self
                    .view()
                    .hunk_rows(&hunks[index], entire_deletion)
                    .is_some_and(|rows| rows.contains(&current_row)))
                .then_some(index)
            })
            .or_else(|| rendered_hunk_index(self.view(), &hunks, current_row, entire_deletion));
        let line = self.view().cursor_source_line().unwrap_or(0);
        let index = stepped_hunk_index(&lines, line, remembered, direction.is_next());
        let Some(index) = index else {
            return false;
        };
        self.land_on_hunk(path, &hunks, index);
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
        self.dismiss_navigation_peek();
        self.focus = Focus::View;
        let hunks = self.view().hunks();
        if hunks.is_empty() {
            self.view_mut().clear_normal_deletion();
            self.view_mut().goto_source_line(1);
            let row = self.view().cursor().row;
            self.view_mut().place_jump_top_third(row);
            self.change_stop = Some(ChangeStop {
                path: path.to_path_buf(),
                hunk: None,
                old: 0..0,
                new: 0..0,
            });
            self.change_placement = Some(ChangePlacement);
            self.synchronize_tree_to(path);
        } else {
            let index = if landing == PathLanding::FirstHunk || direction.is_next() {
                0
            } else {
                hunks.len() - 1
            };
            self.land_on_hunk(path.to_path_buf(), &hunks, index);
        }
        true
    }

    fn land_on_hunk(&mut self, path: PathBuf, hunks: &[Hunk], index: usize) {
        self.close_popup();
        self.open_file_view();
        self.focus = Focus::View;
        let Some(hunk) = hunks.get(index) else {
            return;
        };
        self.dismiss_navigation_peek();
        let entire_deletion = self
            .current
            .and_then(|current| self.docs.get(current))
            .is_some_and(|doc| doc.deleted == Some(super::Deleted::ComparisonBase));
        self.view_mut().show_normal_deletion(hunk, entire_deletion);
        let target = hunk.target_line(self.view().index().line_count());
        self.view_mut().goto_source_line(target);
        if let Some(rows) = self.view().hunk_rows(hunk, entire_deletion) {
            self.view_mut().goto_row(rows.start);
            self.view_mut().place_jump_top_third(rows.start);
        }
        self.synchronize_tree_to(&path);
        self.change_stop = Some(ChangeStop {
            path,
            hunk: Some(index),
            old: hunk.old_range(),
            new: hunk.new_range(),
        });
        self.change_placement = Some(ChangePlacement);
    }

    /// Re-resolve a comparison destination after passive layout changes.
    pub(super) fn restore_change_jump_placement(&mut self) {
        if self.change_placement.is_none() {
            return;
        }
        let Some(stop) = self.change_stop.clone() else {
            return;
        };
        if self.current_path() != stop.path {
            return;
        }
        let Some(index) = stop.hunk else {
            let row = self.view().cursor().row;
            self.view_mut().place_jump_top_third(row);
            return;
        };
        let hunks = self.view().hunks();
        let Some(hunk) = hunks
            .get(index)
            .filter(|hunk| hunk.old_range() == stop.old && hunk.new_range() == stop.new)
        else {
            return;
        };
        let entire_deletion = self
            .current
            .and_then(|current| self.docs.get(current))
            .is_some_and(|doc| doc.deleted == Some(super::Deleted::ComparisonBase));
        self.view_mut().show_normal_deletion(hunk, entire_deletion);
        if let Some(rows) = self.view().hunk_rows(hunk, entire_deletion) {
            self.view_mut().goto_row(rows.start);
            self.view_mut().place_jump_top_third(rows.start);
        }
    }
}

fn rendered_hunk_index(
    view: &super::view::View,
    hunks: &[Hunk],
    row: usize,
    entire_deletion: bool,
) -> Option<usize> {
    if let Some(old_line) = view
        .layout()
        .lines()
        .get(row)
        .and_then(fathomable_core::layout::Line::diff_old_line)
    {
        return hunks
            .iter()
            .position(|hunk| hunk.old_range().contains(&(old_line - 1)));
    }

    hunks.iter().position(|hunk| {
        (!hunk.new_range().is_empty() || view.diff_view())
            && view
                .hunk_rows(hunk, entire_deletion)
                .is_some_and(|rows| rows.contains(&row))
    })
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
mod tests;
