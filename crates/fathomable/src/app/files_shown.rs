// @okf-doc: /decisions/0068-what-the-files-pane-shows.md
//! What the files pane shows (ADR 0068): four session toggles under
//! `Space F`, only changed files (`c`), only files with reviews (`o`), hide
//! untracked files (`u`), and show ignored files (`i`), working from any pane.
//!
//! The rules themselves are the tree's [`Shown`]; this module is the
//! app's hands on them: the toggle, the live label a which-key entry
//! carries so it says what a press does now, the checked state of the
//! Files settings menu, the notice while the pane is hidden, and the
//! compact markers by which the pane's header names the state.

use std::fs;
use std::path::PathBuf;

use fathomable_core::annotations::{Lifecycle, Store};
use fathomable_core::tree::{Rule, Shown, Tree};

use super::App;
use super::input::bindings::{self, Action, Chord, Where};

impl App {
    /// Flip a `Space F` rule on the files pane, shown or hidden, keeping the
    /// cursor's file where it is still listed.
    pub(crate) fn files_toggle(&mut self, rule: Rule) {
        if rule == Rule::Changed && self.diff_mode() == fathomable_core::config::DiffMode::Off {
            self.notice("diff mode is off");
            return;
        }
        if !self.ensure_tree() {
            return;
        }
        self.refresh_review_paths();
        let status = self.files_filter_status();
        let virtual_paths = self.comparison_virtual_paths();
        let snapshot_paths = self.comparison_snapshot_paths();
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let shown = tree.shown().toggled(rule);
        if let Err(error) = tree.set_shown(&mut self.workspace, &status, shown) {
            self.notice(error.to_string());
            return;
        }
        if let Some(paths) = snapshot_paths {
            tree.set_snapshot_paths(&status, paths);
        } else {
            tree.set_virtual_paths(&status, virtual_paths);
        }
        self.refresh_tree_target();
        self.refresh_directory_selection();
        if !self.sidebar.tree {
            // The header that names the state is not on screen.
            let words = shown_words(shown);
            let state = if words.is_empty() {
                "all files".to_owned()
            } else {
                words.join(" ")
            };
            self.notice(format!("File list: {state}"));
        }
    }

    /// A new status landed: the files pane lists by it.
    pub(super) fn sift_tree(&mut self) {
        self.cancel_tree_scan();
        let status = self.files_filter_status();
        let virtual_paths = self.comparison_virtual_paths();
        let snapshot_paths = self.comparison_snapshot_paths();
        let review_paths = self.review_paths();
        if let Some(tree) = self.tree.as_mut() {
            tree.set_review_paths(&mut self.workspace, &status, review_paths);
            if let Some(paths) = snapshot_paths {
                tree.set_snapshot_paths(&status, paths);
            } else {
                tree.set_virtual_paths(&status, virtual_paths);
            }
            self.refresh_tree_target();
            self.refresh_directory_selection();
        }
    }

    /// Re-project qualifying current-workspace threads into tree paths.
    pub(super) fn refresh_review_paths(&mut self) {
        self.cancel_tree_scan();
        if self.tree.is_none() {
            return;
        }
        let status = self.files_filter_status();
        let review_paths = self.review_paths();
        if let Some(tree) = self.tree.as_mut() {
            tree.set_review_paths(&mut self.workspace, &status, review_paths);
            self.refresh_tree_target();
            self.refresh_directory_selection();
        }
    }

    fn review_paths(&self) -> Vec<PathBuf> {
        self.store
            .iter()
            .flat_map(Store::threads)
            .filter(|thread| self.reach.here(thread))
            .filter(|thread| {
                matches!(
                    thread.lifecycle(),
                    Lifecycle::Active | Lifecycle::ResolutionProposed
                )
            })
            .map(|thread| self.thread_path(thread).to_path_buf())
            .collect()
    }

    fn files_filter_status(&self) -> fathomable_core::status::Status {
        if self.diff_mode() == fathomable_core::config::DiffMode::Off
            && self.displayed_target_is_working_tree()
        {
            fathomable_core::status::Status::from_entries(
                self.status
                    .entries()
                    .iter()
                    .filter(|entry| {
                        fs::symlink_metadata(self.workspace.root().join(entry.path())).is_ok()
                    })
                    .cloned()
                    .collect(),
            )
        } else {
            self.comparison_status().clone()
        }
    }

    /// What the files pane lists, `Shown::all()` before the tree exists.
    fn files_shown(&self) -> Shown {
        self.tree.as_ref().map_or_else(Shown::all, Tree::shown)
    }

    /// Stop applying changed-only while retaining its checked state.
    pub(super) fn suspend_changed_filter(&mut self) {
        let shown = self.files_shown();
        if self.dormant_changed_filter || !shown.changed_only() {
            return;
        }
        let status = self.comparison_status().clone();
        if let Some(tree) = self.tree.as_mut() {
            if let Err(error) =
                tree.set_shown(&mut self.workspace, &status, shown.toggled(Rule::Changed))
            {
                self.notice(error.to_string());
                return;
            }
            self.dormant_changed_filter = true;
        }
    }

    /// Reapply the retained changed-only rule after leaving Off.
    pub(super) fn restore_changed_filter(&mut self) {
        if !self.dormant_changed_filter {
            return;
        }
        let shown = self.files_shown();
        let status = self.comparison_status().clone();
        if let Some(tree) = self.tree.as_mut()
            && let Err(error) =
                tree.set_shown(&mut self.workspace, &status, shown.toggled(Rule::Changed))
        {
            self.notice(error.to_string());
            return;
        }
        self.dormant_changed_filter = false;
    }

    /// What pressing a toggle's key does now, when that differs from the
    /// table's label: `all files` while only changed or review-bearing files
    /// are listed, `show untracked` while they are hidden, `hide ignored`
    /// while they are shown.
    pub(crate) fn live_label(&self, action: Action) -> Option<&'static str> {
        let shown = self.files_shown();
        match action {
            Action::FilesChanged if shown.changed_only() => Some("all files"),
            Action::FilesReviews if shown.reviews_only() => Some("all files"),
            Action::FilesUntracked if !shown.untracked() => Some("show untracked"),
            Action::FilesIgnored if shown.ignored() => Some("hide ignored"),
            _ => None,
        }
    }

    /// The stable state label used by the Files settings menu.
    pub(crate) fn files_setting_label(action: Action) -> &'static str {
        match action {
            Action::FilesChanged => "only changed",
            Action::FilesReviews => "only reviews",
            Action::FilesUntracked => "hide untracked",
            Action::FilesIgnored => "show ignored",
            _ => "",
        }
    }

    /// Whether a Files setting is active and should carry a checkmark.
    pub(crate) fn files_setting_checked(&self, action: Action) -> bool {
        let shown = self.files_shown();
        match action {
            Action::FilesChanged => shown.changed_only() || self.dormant_changed_filter,
            Action::FilesReviews => shown.reviews_only(),
            Action::FilesUntracked => !shown.untracked(),
            Action::FilesIgnored => shown.ignored(),
            _ => false,
        }
    }

    /// The which-key entries for the keys typed so far on `place`, each
    /// toggle relabelled with what pressing it does now.
    #[cfg(test)]
    pub(crate) fn which_key(&self, place: Where) -> Vec<(Chord, String)> {
        bindings::menu_entries(place, self.prefix(), |action| self.live_label(action))
    }

    /// The drawn which-key sections, grouped by related actions.
    pub(crate) fn which_key_sections(&self, place: Where) -> Vec<bindings::MenuSection> {
        let mut sections =
            bindings::menu_sections(place, self.prefix(), |action| self.live_label(action));
        let active = match self.diff_mode() {
            fathomable_core::config::DiffMode::Standard => Action::DiffStandard,
            fathomable_core::config::DiffMode::Unified => Action::DiffUnified,
            fathomable_core::config::DiffMode::Off => Action::DiffOff,
        };
        for section in &mut sections {
            section.select(active);
        }
        sections
    }

    /// Whether a visible which-key route is currently actionable.
    pub(crate) fn which_key_enabled(&self, place: Where, chord: Chord) -> bool {
        let mut keys = self.prefix().to_vec();
        keys.push(chord);
        let bindings::Match::Exact(action) = bindings::lookup(place, &keys) else {
            return true;
        };
        self.diff_mode() != fathomable_core::config::DiffMode::Off
            || !matches!(action, Action::ComparisonWhitespace | Action::FilesChanged)
    }

    /// The compact marker the files pane's header uses for active rules.
    pub(crate) fn files_shown_marker(&self) -> String {
        let shown = self.files_shown();
        let mut markers = Vec::new();
        if shown.changed_only() && self.diff_mode() != fathomable_core::config::DiffMode::Off {
            markers.push("c");
        }
        if shown.reviews_only() {
            markers.push("r");
        }
        if !shown.untracked() {
            markers.push("u");
        }
        if shown.ignored() {
            markers.push("i");
        }
        markers.join(",")
    }
}

/// Words saying what the active filters put on screen.
fn shown_words(shown: Shown) -> Vec<&'static str> {
    let mut words = Vec::new();
    if shown.changed_only() {
        words.push("changed");
    }
    if shown.reviews_only() {
        words.push("reviews");
    }
    if !shown.untracked() {
        words.push("tracked");
    }
    if shown.ignored() {
        words.push("ignored");
    }
    words
}

#[cfg(test)]
mod tests;
